//! Our reminders: a scheduler that, when the time comes, speaks on the station without a question (as the debug
//! API's say). That speech is not yet checked as a reminder (experiment 2), so reminders go to Yandex (its push)
//! while Yandex is on the route; ROUTES=reminders=own makes them ours.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration as StdDuration;

use bytes::Bytes;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use futures::future::BoxFuture;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::alarms_skill::{zoned_clock, DEFAULT_TIMEZONE};
use super::clock_skill::{say_day_and_time, wall_clock};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};
use crate::skills::text::{normalize, parse_clock, parse_duration, read_number, say_duration};
use crate::skills::typed_directives::new_timer_id;
use crate::storage::StateStore;
use crate::utils::time::now_ms;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredReminder {
    pub id: String,
    /// When to say it, ms since the epoch.
    pub at: i64,
    pub text: String,
}

/// Recorded audio played after the words (a contact's voice message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioClip {
    pub audio: Bytes,
    /// For the answer's text and the journal.
    pub label: String,
}

/// Speaks a text on a device now (`*`: on whichever station is connected), with recorded audio after it if given;
/// false when no such station is connected (or it could not speak).
pub trait Speaker: Send + Sync {
    fn say(&self, device_tag: String, text: String, clip: Option<AudioClip>) -> BoxFuture<'static, bool>;
}

const CHECK_MS: u64 = 15_000;
/// A reminder the device was offline for is tried this long, then dropped.
const GIVE_UP_MS: i64 = 6 * 3_600_000;

static DURATION_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^([0-9]+|полчаса|полтора|полторы|час|часа|часов|минуту|минуты|минут|секунду|секунды|секунд|и)$")
        .expect("a valid pattern")
});
static CLOCK_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^([0-9]+|[0-9]{1,2}:[0-9]{2}|час|часа|часов|утра|вечера|дня|ночи|минут|минуты)$")
        .expect("a valid pattern")
});
static ABOUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(что|чтобы|о|об|про)\s+").expect("a valid pattern"));

/// When and what to remind of.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedReminder {
    /// Milliseconds since the epoch, as the TS computed them, in floating point: a duration said in any number of
    /// hours neither overflows nor wraps into the past, and `set` turns down a moment no Date holds.
    pub at: f64,
    pub text: String,
}

/// Milliseconds since the epoch of a wall-clock time in a day `day_shift` days on, at an offset.
fn wall_ms(year: i32, month: u32, day: u32, day_shift: i64, hours: u32, minutes: u32, offset_seconds: i32) -> i64 {
    let date = NaiveDate::from_ymd_opt(year, month, day).unwrap_or_default() + Duration::days(day_shift);
    date.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc().timestamp_millis()
        + (i64::from(hours) * 60 + i64::from(minutes)) * 60_000
        - i64::from(offset_seconds) * 1000
}

/// "через 5 минут выключить чайник": the time and what to remind of; None without a time.
pub fn parse_reminder(request: &str, now: DateTime<Utc>, timezone: &str) -> Option<ParsedReminder> {
    let normalized = normalize(request);
    let words: Vec<&str> = normalized.split(' ').collect();
    let is_number = |index: usize| read_number(&words, index).is_some();
    let now_ms = now.timestamp_millis();
    if words[0] == "через" {
        let mut end = 1;
        while end < words.len() && (DURATION_WORD.is_match(words[end]) || is_number(end)) {
            end += 1;
        }
        let seconds = parse_duration(&words[1..end].join(" ")).filter(|seconds| *seconds > 0)?;
        let text = ABOUT.replace(&words[end..].join(" "), "").into_owned();
        if text.is_empty() {
            return None;
        }
        return Some(ParsedReminder { at: now_ms as f64 + seconds as f64 * 1000.0, text });
    }
    let (start, day_shift) = match words[0] {
        "завтра" => (1, 1),
        "сегодня" => (1, 0),
        _ => (0, 0),
    };
    if words.get(start) != Some(&"в") {
        return None;
    }
    let mut end = start + 1;
    while end < words.len() && (CLOCK_WORD.is_match(words[end]) || is_number(end)) {
        end += 1;
    }
    let time = parse_clock(&words[start + 1..end].join(" "))?;
    let text = ABOUT.replace(&words[end..].join(" "), "").into_owned();
    if text.is_empty() {
        return None;
    }
    let clock = zoned_clock(now, timezone);
    let mut at = wall_ms(clock.year, clock.month, clock.day, day_shift, time.hours, time.minutes, clock.offset_seconds);
    if at <= now_ms && day_shift == 0 {
        at += 86_400_000;
    }
    Some(ParsedReminder { at: at as f64, text })
}

pub struct RemindersSkill {
    store: StateStore<StoredReminder>,
    speaker: RwLock<Option<Arc<dyn Speaker>>>,
    delivering: AtomicBool,
    stop: CancellationToken,
}

impl RemindersSkill {
    pub fn new(store: StateStore<StoredReminder>) -> RemindersSkill {
        RemindersSkill {
            store,
            speaker: RwLock::new(None),
            delivering: AtomicBool::new(false),
            stop: CancellationToken::new(),
        }
    }

    /// How reminders are said: the session registry gives the live station of a device.
    pub fn attach_speaker(&self, speaker: Arc<dyn Speaker>) {
        *self.speaker.write().unwrap() = Some(speaker);
    }

    /// Checks for due reminders every 15 s until `stop`; inside the runtime.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let reminders = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(StdDuration::from_millis(CHECK_MS));
            ticks.tick().await;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = ticks.tick() => {}
                }
                let Some(reminders) = reminders.upgrade() else { break };
                reminders.deliver().await;
            }
        })
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    fn run(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = context.device_tag.as_str();
        let now = context.now;
        let timezone = context.timezone.as_deref().filter(|zone| !zone.is_empty()).unwrap_or(DEFAULT_TIMEZONE);
        Ok(match intent.action.as_str() {
            "set" => {
                let Some(parsed) = self.parse(&intent.params, now, timezone) else {
                    return Ok(Outcome::speech(
                        "Скажите, когда напомнить: например, напомни через десять минут выключить чайник.",
                    ));
                };
                // A moment no Date holds (any number of hours may be said): the TS's Intl threw «Invalid time
                // value» for it, though only after keeping the reminder; nothing is kept here.
                let Some(at) = js::date(parsed.at).map(|moment| moment.timestamp_millis()) else {
                    anyhow::bail!("Invalid time value");
                };
                let Some(clock) = at_clock(at, timezone) else { anyhow::bail!("Invalid time value") };
                let mut reminders = self.store.get(device);
                reminders.push(StoredReminder { id: new_timer_id(), at, text: parsed.text.clone() });
                self.store.set(device, reminders);
                let in_seconds = js::round((at - now.timestamp_millis()) as f64 / 1000.0) as i64;
                let when = if in_seconds < 6 * 3600 {
                    format!("через {}", say_duration(in_seconds, true))
                } else {
                    format!("в {}", say_day_and_time(&clock))
                };
                Outcome::speech(format!("Хорошо, напомню {when}."))
            }
            "list" => {
                let mut reminders = self.store.get(device);
                if reminders.is_empty() {
                    return Ok(Outcome::speech("Напоминаний нет."));
                }
                reminders.sort_by_key(|reminder| reminder.at);
                let said: Option<Vec<String>> = reminders
                    .iter()
                    .map(|reminder| {
                        let clock = at_clock(reminder.at, timezone)?;
                        Some(format!("{} — {}", say_day_and_time(&clock), reminder.text))
                    })
                    .collect();
                let Some(said) = said else { anyhow::bail!("Invalid time value") };
                Outcome::speech(format!("Напоминания: {}.", said.join("; ")))
            }
            "cancel" => {
                self.store.set(device, Vec::new());
                Outcome::speech("Отменила напоминания.")
            }
            other => anyhow::bail!("reminders: unknown action {other}"),
        })
    }

    fn parse(&self, params: &Map<String, Value>, now: DateTime<Utc>, timezone: &str) -> Option<ParsedReminder> {
        if let Some(request) = js::string_param(params, "request") {
            return parse_reminder(request, now, timezone);
        }
        let text = js::string_param(params, "text").filter(|text| !text.is_empty())?;
        if let Some(in_seconds) = js::number_param(params, "inSeconds") {
            return Some(ParsedReminder { at: now.timestamp_millis() as f64 + in_seconds * 1000.0, text: text.into() });
        }
        let at = js::string_param(params, "at")?;
        let Some(date) = js::string_param(params, "date") else {
            return parse_reminder(&format!("в {at} {text}"), now, timezone);
        };
        let numbers =
            |text: &str, separator: char| -> Vec<f64> { text.split(separator).map(js::string_to_number).collect() };
        let (day, time) = (numbers(date, '-'), numbers(at, ':'));
        if day.len() < 3 || time.len() < 2 || day.iter().chain(&time).any(|n| !n.is_finite()) {
            return None;
        }
        let clock = zoned_clock(now, timezone);
        let midnight = NaiveDate::from_ymd_opt(day[0] as i32, day[1] as u32, day[2] as u32)?;
        let at = midnight.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64
            + (time[0] * 60.0 + time[1]) * 60_000.0
            - f64::from(clock.offset_seconds) * 1000.0;
        Some(ParsedReminder { at, text: text.into() })
    }

    /// Says what is due; a device that is offline is tried again until GIVE_UP_MS.
    pub async fn deliver(&self) {
        let Some(speaker) = self.speaker.read().unwrap().clone() else { return };
        if self.delivering.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = now_ms() as i64;
        for device in self.store.devices() {
            let due: Vec<StoredReminder> = self.store.get(&device).into_iter().filter(|r| r.at <= now).collect();
            if due.is_empty() {
                continue;
            }
            let mut said = Vec::new();
            for reminder in due {
                if speaker.say(device.clone(), format!("Напоминаю: {}.", reminder.text), None).await {
                    said.push(reminder.id);
                } else if now - reminder.at > GIVE_UP_MS {
                    tracing::warn!(
                        target: "reminders",
                        "reminder \"{}\" for {device}: the station was offline for {} h, dropped",
                        reminder.text,
                        GIVE_UP_MS / 3_600_000
                    );
                    said.push(reminder.id);
                }
            }
            if !said.is_empty() {
                self.store
                    .set(&device, self.store.get(&device).into_iter().filter(|r| !said.contains(&r.id)).collect());
            }
        }
        self.delivering.store(false, Ordering::SeqCst);
    }
}

/// The wall clock of a moment where the station is; None where no Date holds it, as no Intl formats it.
fn at_clock(at: i64, timezone: &str) -> Option<NaiveDateTime> {
    wall_clock(js::date(at as f64)?, Some(timezone))
}

impl OwnSkill for RemindersSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Reminders
    }

    fn verified(&self, _intent: &Intent) -> bool {
        false
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { self.run(intent, context) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use serde_json::json;
    use std::sync::Mutex;

    fn now() -> DateTime<Utc> {
        // 14:00 in Dubai.
        "2026-11-01T10:00:00Z".parse().unwrap()
    }

    fn utc(year: i32, month: u32, day: u32, hours: u32, minutes: u32) -> f64 {
        wall_ms(year, month, day, 0, hours, minutes, 0) as f64
    }

    #[test]
    fn reads_when_and_what() {
        let base = now().timestamp_millis() as f64;
        let parsed = |text: &str| parse_reminder(text, now(), "Asia/Dubai");
        assert_eq!(
            parsed("через 5 минут выключить чайник"),
            Some(ParsedReminder { at: base + 300_000.0, text: "выключить чайник".into() })
        );
        assert_eq!(
            parsed("через минуту что пора идти"),
            Some(ParsedReminder { at: base + 60_000.0, text: "пора идти".into() })
        );
        assert_eq!(
            parsed("завтра в 9 утра купить хлеб"),
            Some(ParsedReminder { at: utc(2026, 11, 2, 5, 0), text: "купить хлеб".into() })
        );
        assert_eq!(
            parsed("в 15:30 позвонить маме"),
            Some(ParsedReminder { at: utc(2026, 11, 1, 11, 30), text: "позвонить маме".into() })
        );
        assert_eq!(parsed("купить хлеб"), None);
    }

    struct Station {
        online: AtomicBool,
        said: Mutex<Vec<String>>,
    }

    impl Speaker for Arc<Station> {
        fn say(&self, _device: String, text: String, _clip: Option<AudioClip>) -> BoxFuture<'static, bool> {
            let online = self.online.load(Ordering::SeqCst);
            if online {
                self.said.lock().unwrap().push(text);
            }
            Box::pin(async move { online })
        }
    }

    #[tokio::test]
    async fn says_a_due_reminder_on_the_station_and_keeps_one_for_an_offline_station() {
        let skill = RemindersSkill::new(StateStore::open(Arc::new(Db::memory()), "reminders"));
        let station = Arc::new(Station { online: AtomicBool::new(false), said: Mutex::new(Vec::new()) });
        skill.attach_speaker(Arc::new(station.clone()));
        let mut context = ExecutionContext::new("h:test", Utc::now() - Duration::seconds(2));
        context.timezone = Some("Asia/Dubai".into());
        let intent = Intent {
            capability: CapabilityName::Reminders,
            action: "set".into(),
            params: json!({"text": "выключить чайник", "inSeconds": 1}).as_object().cloned().unwrap(),
        };
        let set = skill.execute(&intent, &context).await.unwrap();
        assert_eq!(set.speech.as_deref(), Some("Хорошо, напомню через 1 секунду."));
        skill.deliver().await;
        assert_eq!(skill.store.get("h:test").len(), 1);
        station.online.store(true, Ordering::SeqCst);
        skill.deliver().await;
        assert_eq!(*station.said.lock().unwrap(), ["Напоминаю: выключить чайник."]);
        assert!(skill.store.get("h:test").is_empty());
        skill.stop();
    }

    #[tokio::test]
    async fn lists_and_cancels_with_the_day_and_time_where_the_station_is() {
        let skill = RemindersSkill::new(StateStore::open(Arc::new(Db::memory()), "reminders"));
        let mut context = ExecutionContext::new("h:test", now());
        context.timezone = Some("Asia/Dubai".into());
        let set = |params: Value| Intent {
            capability: CapabilityName::Reminders,
            action: "set".into(),
            params: params.as_object().cloned().unwrap(),
        };
        let later = skill.execute(&set(json!({"request": "завтра в 9 утра купить хлеб"})), &context).await.unwrap();
        assert_eq!(later.speech.as_deref(), Some("Хорошо, напомню в 2 ноября в 09:00."));
        let dated =
            skill.execute(&set(json!({"text": "полить цветы", "at": "15:30", "date": "2026-11-01"})), &context).await;
        assert_eq!(dated.unwrap().speech.as_deref(), Some("Хорошо, напомню через 1 час 30 минут."));
        let list = skill.execute(&Intent::new(CapabilityName::Reminders, "list"), &context).await.unwrap();
        assert_eq!(
            list.speech.as_deref(),
            Some("Напоминания: 1 ноября в 15:30 — полить цветы; 2 ноября в 09:00 — купить хлеб.")
        );
        let cancel = skill.execute(&Intent::new(CapabilityName::Reminders, "cancel"), &context).await.unwrap();
        assert_eq!(cancel.speech.as_deref(), Some("Отменила напоминания."));
        assert!(skill.store.get("h:test").is_empty());
    }

    #[tokio::test]
    async fn fails_a_reminder_no_date_holds_as_the_ts_did_and_keeps_nothing() {
        let skill = RemindersSkill::new(StateStore::open(Arc::new(Db::memory()), "reminders"));
        let mut context = ExecutionContext::new("h:test", now());
        context.timezone = Some("Europe/Moscow".into());
        let set = |params: Value| Intent {
            capability: CapabilityName::Reminders,
            action: "set".into(),
            params: params.as_object().cloned().unwrap(),
        };
        // An hour before chrono's last moment, which Moscow's clock has passed.
        let edge = DateTime::<Utc>::MAX_UTC.timestamp() - 3600 - now().timestamp();
        let requests = [
            // Past i64 in ms.
            "через 10000000000000 часов выпить чай".to_string(),
            // Past what a Date holds.
            "через 3000000000 часов выпить чай".to_string(),
            format!("через {edge} секунд выпить чай"),
        ];
        for request in requests {
            let failed = skill.execute(&set(json!({ "request": request })), &context).await;
            assert_eq!(failed.unwrap_err().to_string(), "Invalid time value", "{request}");
        }
        // The model's clock time on a date, past any Date.
        let dated = json!({"text": "полить цветы", "at": "99999999999999:00", "date": "2026-11-01"});
        assert_eq!(skill.execute(&set(dated), &context).await.unwrap_err().to_string(), "Invalid time value");
        assert!(skill.store.get("h:test").is_empty());
        // A long way off, but a Date: the day and the time, as before.
        let later = skill.execute(&set(json!({"request": "через 2000000 часов выпить чай"})), &context).await;
        assert_eq!(later.unwrap().speech.as_deref(), Some("Хорошо, напомню в 29 декабря в 21:00."));
    }
}
