//! Alarms: the station keeps and rings them, and lists them in every request (its alarm capability). The list and
//! «стоп» while one rings are ours; setting, cancelling and snoozing go to Yandex while it is on the route, as its
//! alarms live in the account and the app shows and edits them. With ROUTES=alarms=own they are ours too, in the
//! shapes Yandex sends (relay journal, 02.11).

use chrono::{DateTime, Datelike, Duration, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::gateway::protocol::protocol_types::{DeviceState, StationAlarm};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill};
use crate::skills::text::parse_clock;
use crate::skills::typed_directives::{
    alarm_add, alarm_cancel, alarm_snooze, alarm_stop, new_timer_id, AlarmSpec, CalendarDate,
};
use crate::storage::StateStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmDay {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAlarm {
    pub id: String,
    pub hours: u32,
    pub minutes: u32,
    /// The day it rings (one-time alarms), in the station's time zone.
    pub date: AlarmDay,
    pub timezone: String,
}

const RINGING_MS: i64 = 15 * 60_000;
const ASK_MS: u64 = 30_000;
const SNOOZE_S: i64 = 600;
/// Where the station says nothing of its time zone.
pub const DEFAULT_TIMEZONE: &str = "Europe/Moscow";

/// The wall clock in a time zone: date, time and its UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZonedClock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hours: u32,
    pub minutes: u32,
    pub offset_seconds: i32,
}

/// The wall clock of `now` in a time zone; a zone the gateway does not know reads as UTC.
pub fn zoned_clock(now: DateTime<Utc>, timezone: &str) -> ZonedClock {
    match timezone.parse::<Tz>() {
        Ok(zone) => clock_of(&now.with_timezone(&zone)),
        Err(_) => clock_of(&now),
    }
}

/// The fields of the zoned moment itself, not of its `naive_local()`: that panics where the local time is past an end
/// of chrono's range, as it is within hours of one in a zone off UTC.
fn clock_of<Z: TimeZone>(local: &DateTime<Z>) -> ZonedClock {
    ZonedClock {
        year: local.year(),
        month: local.month(),
        day: local.day(),
        hours: local.hour(),
        minutes: local.minute(),
        offset_seconds: local.offset().fix().local_minus_utc(),
    }
}

/// Milliseconds since the epoch of a wall-clock time at an offset.
fn utc_ms(year: i32, month: u32, day: u32, hours: u32, minutes: u32, offset_seconds: i32) -> i64 {
    let date = NaiveDate::from_ymd_opt(year, month, day).unwrap_or_default();
    let local = date.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc().timestamp_millis();
    local + (i64::from(hours) * 60 + i64::from(minutes)) * 60_000 - i64::from(offset_seconds) * 1000
}

/// When an alarm rings, as a UTC instant.
fn rings_at(alarm: &StoredAlarm, offset_seconds: i32) -> i64 {
    utc_ms(alarm.date.year, alarm.date.month, alarm.date.day, alarm.hours, alarm.minutes, offset_seconds)
}

/// The days from one date to another.
fn days_between(from: (i32, u32, u32), to: (i32, u32, u32)) -> i64 {
    let date = |(year, month, day): (i32, u32, u32)| NaiveDate::from_ymd_opt(year, month, day).unwrap_or_default();
    (date(to) - date(from)).num_days()
}

/// An alarm as said: «7:05», «7:05 завтра», by the day it rings in the station's time zone.
fn say_alarm(hours: u32, minutes: u32, date: Option<(i32, u32, u32)>, now: DateTime<Utc>, timezone: &str) -> String {
    let time = format!("{hours}:{minutes:02}");
    let Some(date) = date else { return time };
    let today = zoned_clock(now, timezone);
    if days_between((today.year, today.month, today.day), date) == 1 {
        format!("{time} завтра")
    } else {
        time
    }
}

/// An alarm to say or cancel: the station's or ours.
struct Upcoming {
    id: String,
    hours: u32,
    minutes: u32,
    date: Option<(i32, u32, u32)>,
}

/// The station's alarms still to ring, earliest first; repeating ones (no date) after the one-time ones.
fn station_upcoming(state: &DeviceState) -> Vec<Upcoming> {
    let mut alarms: Vec<&StationAlarm> = state.alarms.iter().flatten().filter(|alarm| !alarm.rung).collect();
    alarms.sort_by_key(|alarm| match alarm.date {
        Some(date) => (false, utc_ms(date.year, date.month, date.day, alarm.hours, alarm.minutes, 0)),
        None => (true, 0),
    });
    alarms
        .into_iter()
        .map(|alarm| Upcoming {
            id: alarm.id.clone(),
            hours: alarm.hours,
            minutes: alarm.minutes,
            date: alarm.date.map(|date| (date.year, date.month, date.day)),
        })
        .collect()
}

pub struct AlarmsSkill {
    store: StateStore<StoredAlarm>,
}

impl AlarmsSkill {
    pub fn new(store: StateStore<StoredAlarm>) -> AlarmsSkill {
        AlarmsSkill { store }
    }

    /// The alarm that rings now: by the station's own word when it lists its alarms, otherwise ours whose time came
    /// within the last minutes.
    pub fn ringing(&self, device: &str, state: Option<&DeviceState>, now: DateTime<Utc>) -> Option<String> {
        if let Some(alarms) = state.and_then(|state| state.alarms.as_ref()) {
            return alarms.iter().find(|alarm| alarm.ringing).map(|alarm| alarm.id.clone());
        }
        let now_ms = now.timestamp_millis();
        self.store
            .get(device)
            .into_iter()
            .find(|alarm| {
                let at = rings_at(alarm, zoned_clock(now, &alarm.timezone).offset_seconds);
                at <= now_ms && now_ms - at < RINGING_MS
            })
            .map(|alarm| alarm.id)
    }

    fn run(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = context.device_tag.as_str();
        let timezone = context.timezone.as_deref().filter(|zone| !zone.is_empty()).unwrap_or(DEFAULT_TIMEZONE);
        let state = context.device_state.as_ref().filter(|state| state.alarms.is_some());
        Ok(match intent.action.as_str() {
            "set" => self.set(device, &intent.params, context.now, timezone)?,
            "continue" => match parse_clock(&js::string_or_empty(intent.params.get("text"))) {
                None => Outcome::speech("Не поняла время. Скажите, например: будильник на семь тридцать."),
                Some(time) => {
                    let mut params = Map::new();
                    params.insert("hours".into(), Value::from(time.hours));
                    params.insert("minutes".into(), Value::from(time.minutes));
                    self.set(device, &params, context.now, timezone)?
                }
            },
            "cancel" => {
                let wanted = js::number_param(&intent.params, "hours");
                let wanted_minutes = js::number_param(&intent.params, "minutes").unwrap_or(0.0);
                let at = |hours: u32, minutes: u32| {
                    wanted.is_none_or(|wanted| f64::from(hours) == wanted && f64::from(minutes) == wanted_minutes)
                };
                let alarms = match state {
                    Some(state) => station_upcoming(state),
                    None => self.upcoming(device, context.now).iter().map(Upcoming::from).collect(),
                };
                let ids: Vec<String> =
                    alarms.into_iter().filter(|alarm| at(alarm.hours, alarm.minutes)).map(|alarm| alarm.id).collect();
                if ids.is_empty() {
                    return Ok(Outcome::speech("Таких будильников нет."));
                }
                self.store
                    .set(device, self.store.get(device).into_iter().filter(|alarm| !ids.contains(&alarm.id)).collect());
                Outcome::said(
                    if ids.len() > 1 {
                        "Отменила будильники."
                    } else {
                        "Отменила будильник."
                    },
                    ids.iter().map(|id| alarm_cancel(id)).collect(),
                )
            }
            "status" => {
                let alarms = match state {
                    Some(state) => station_upcoming(state),
                    None => self.upcoming(device, context.now).iter().map(Upcoming::from).collect(),
                };
                if alarms.is_empty() {
                    return Ok(Outcome::speech("Будильников нет."));
                }
                let said: Vec<String> = alarms
                    .iter()
                    .map(|alarm| {
                        format!("на {}", say_alarm(alarm.hours, alarm.minutes, alarm.date, context.now, timezone))
                    })
                    .collect();
                Outcome::speech(format!(
                    "{} {}.",
                    if alarms.len() > 1 { "Будильники" } else { "Будильник" },
                    said.join(", ")
                ))
            }
            "stop" => Outcome::directives(vec![alarm_stop()]),
            "snooze" => self.snooze(device, state, context.now, timezone),
            other => anyhow::bail!("alarms: unknown action {other}"),
        })
    }

    /// As Yandex does it: the ringing stops and a one-time alarm rings ten minutes later.
    fn snooze(&self, device: &str, state: Option<&DeviceState>, now: DateTime<Utc>, timezone: &str) -> Outcome {
        let Some(ringing) = self.ringing(device, state, now) else {
            return match state {
                Some(_) => Outcome::speech("Сейчас будильник не звонит."),
                None => Outcome::directives(vec![alarm_stop()]),
            };
        };
        let original = state.and_then(|state| state.alarms.as_ref()?.iter().find(|alarm| alarm.id == ringing));
        let at = zoned_clock(now + Duration::seconds(SNOOZE_S), timezone);
        let directives = alarm_snooze(&AlarmSpec {
            id: String::new(),
            hours: at.hours,
            minutes: at.minutes,
            seconds: 0,
            timezone: timezone.to_string(),
            offset_seconds: at.offset_seconds,
            date: Some(CalendarDate { year: at.year as u32, month: at.month, day: at.day }),
            weekdays: Vec::new(),
            edit_timestamp: original.and_then(|alarm| alarm.edit_timestamp).map(|stamp| stamp as u64),
        });
        Outcome::said(format!("Хорошо, следующий будильник в {}:{:02}.", at.hours, at.minutes), directives.into())
    }

    fn set(
        &self,
        device: &str,
        params: &Map<String, Value>,
        now: DateTime<Utc>,
        timezone: &str,
    ) -> anyhow::Result<Outcome> {
        let clock = zoned_clock(now, timezone);
        let (hours, minutes) = if let Some(in_seconds) = js::number_param(params, "inSeconds") {
            // The quick rule takes any number of hours. Where no Date holds the moment, the TS's Intl threw «Invalid
            // time value» and the request failed alone; chrono's `+` would panic, and take the gateway with it.
            let Some(at) = js::date(now.timestamp_millis() as f64 + in_seconds * 1000.0) else {
                anyhow::bail!("Invalid time value");
            };
            let at = zoned_clock(at, timezone);
            (at.hours, at.minutes)
        } else if let Some(hours) = js::number_param(params, "hours") {
            (hours as u32, js::number_param(params, "minutes").unwrap_or(0.0) as u32)
        } else {
            return Ok(Outcome {
                speech: Some("На какое время поставить будильник?".into()),
                listen: true,
                continuation: Some(ContinuationRequest { capability: CapabilityName::Alarms, ttl_ms: ASK_MS }),
                ..Outcome::default()
            });
        };
        // Today if the time is still ahead, otherwise tomorrow.
        let later = hours * 60 + minutes > clock.hours * 60 + clock.minutes;
        let today = NaiveDate::from_ymd_opt(clock.year, clock.month, clock.day).unwrap_or_default();
        let day = if later { today } else { today + Duration::days(1) };
        let alarm = StoredAlarm {
            id: new_timer_id(),
            hours,
            minutes,
            date: AlarmDay { year: day.year(), month: day.month(), day: day.day() },
            timezone: timezone.to_string(),
        };
        let mut alarms = self.upcoming(device, now);
        alarms.push(alarm.clone());
        self.store.set(device, alarms);
        let directive = alarm_add(&AlarmSpec {
            id: alarm.id,
            hours,
            minutes,
            seconds: 0,
            timezone: alarm.timezone,
            offset_seconds: clock.offset_seconds,
            date: Some(CalendarDate { year: day.year() as u32, month: day.month(), day: day.day() }),
            weekdays: Vec::new(),
            edit_timestamp: None,
        });
        Ok(Outcome::said(
            format!("Будильник на {hours}:{minutes:02}{}.", if later { "" } else { " завтра" }),
            vec![directive],
        ))
    }

    /// Ours still to ring, or ringing.
    fn upcoming(&self, device: &str, now: DateTime<Utc>) -> Vec<StoredAlarm> {
        let now_ms = now.timestamp_millis();
        self.store
            .get(device)
            .into_iter()
            .filter(|alarm| rings_at(alarm, zoned_clock(now, &alarm.timezone).offset_seconds) + RINGING_MS > now_ms)
            .collect()
    }
}

impl From<&StoredAlarm> for Upcoming {
    fn from(alarm: &StoredAlarm) -> Upcoming {
        Upcoming {
            id: alarm.id.clone(),
            hours: alarm.hours,
            minutes: alarm.minutes,
            date: Some((alarm.date.year, alarm.date.month, alarm.date.day)),
        }
    }
}

impl OwnSkill for AlarmsSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Alarms
    }

    fn verified(&self, intent: &Intent) -> bool {
        intent.action == "status" || intent.action == "stop"
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
    use crate::gateway::protocol::pb;
    use crate::gateway::protocol::protocol_types::AlarmDate;
    use crate::storage::Db;
    use prost::Message;
    use serde_json::json;
    use std::sync::Arc;

    fn skill() -> AlarmsSkill {
        AlarmsSkill::new(StateStore::open(Arc::new(Db::memory()), "alarms"))
    }

    fn intent(action: &str, params: Value) -> Intent {
        Intent {
            capability: CapabilityName::Alarms,
            action: action.into(),
            params: params.as_object().cloned().unwrap(),
        }
    }

    fn context(now: &str, timezone: &str) -> ExecutionContext {
        let mut context = ExecutionContext::new("h:test", now.parse().unwrap());
        context.timezone = Some(timezone.into());
        context
    }

    fn added(outcome: &Outcome, index: usize) -> pb::AlarmSpec {
        pb::AlarmAddPayload::decode(outcome.directives[index].typed_payload.clone().unwrap()).unwrap().alarm.unwrap()
    }

    #[tokio::test]
    async fn sets_a_one_time_alarm_for_the_next_such_time_where_the_station_is() {
        let skill = skill();
        // 10:00 UTC is 14:00 in Dubai (UTC+4): 7:30 is tomorrow.
        let now = context("2026-11-01T10:00:00Z", "Asia/Dubai");
        let outcome = skill.execute(&intent("set", json!({"hours": 7, "minutes": 30})), &now).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Будильник на 7:30 завтра."));
        let alarm = added(&outcome, 0);
        assert_eq!(alarm.local_time, Some(pb::LocalTime { hours: 7, minutes: 30, seconds: 0 }));
        assert_eq!(
            alarm.timezone,
            Some(pb::AlarmTimezone { timezone_name: "Asia/Dubai".into(), offset_seconds: 14400 })
        );
        assert_eq!(alarm.activation_days[0].date, Some(pb::CalendarDate { year: 2026, month: 11, day: 2 }));
        let status = skill.execute(&intent("status", json!({})), &now).await.unwrap();
        assert_eq!(status.speech.as_deref(), Some("Будильник на 7:30 завтра."));
    }

    // With the alarms the station lists (relay journal, 02.11).

    const NOW: &str = "2026-11-02T00:09:07Z"; // 7:09 in Bangkok (UTC+7)

    fn listed(id: &str, hours: u32, minutes: u32) -> StationAlarm {
        StationAlarm {
            id: id.into(),
            hours,
            minutes,
            seconds: 0,
            date: Some(AlarmDate { year: 2026, month: 11, day: 2 }),
            timezone: None,
            offset_seconds: None,
            edit_timestamp: None,
            ringing: false,
            rung: false,
        }
    }

    fn station(alarms: Vec<StationAlarm>) -> ExecutionContext {
        let mut context = context(NOW, "Asia/Bangkok");
        context.device_state = Some(DeviceState { alarms: Some(alarms), ..DeviceState::default() });
        context
    }

    fn the_listed() -> Vec<StationAlarm> {
        vec![
            StationAlarm { rung: true, ..listed("rang", 7, 1) },
            StationAlarm { date: Some(AlarmDate { year: 2026, month: 11, day: 3 }), ..listed("tomorrow", 6, 30) },
            listed("eight", 8, 0),
        ]
    }

    #[tokio::test]
    async fn tells_the_alarms_still_to_ring_by_the_station_not_by_ours() {
        let outcome = skill().execute(&intent("status", json!({})), &station(the_listed())).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Будильники на 8:00, на 6:30 завтра."));
    }

    #[tokio::test]
    async fn cancels_the_one_at_the_time_said() {
        let outcome = skill()
            .execute(&intent("cancel", json!({"hours": 8, "minutes": 0})), &station(the_listed()))
            .await
            .unwrap();
        let ids: Vec<String> = outcome
            .directives
            .iter()
            .map(|d| pb::AlarmIdPayload::decode(d.typed_payload.clone().unwrap()).unwrap().alarm_id)
            .collect();
        assert_eq!(ids, ["eight"]);
    }

    #[tokio::test]
    async fn snoozes_the_ringing_one_as_yandex_does_stop_and_a_one_time_alarm_ten_minutes_later() {
        let skill = skill();
        let ringing =
            station(vec![StationAlarm { ringing: true, edit_timestamp: Some(1793578022), ..listed("ringing", 7, 8) }]);
        assert_eq!(
            skill.ringing("h:test", ringing.device_state.as_ref(), NOW.parse().unwrap()).as_deref(),
            Some("ringing")
        );
        let outcome = skill.execute(&intent("snooze", json!({})), &ringing).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Хорошо, следующий будильник в 7:19."));
        assert_eq!(
            outcome.directives.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["alarm_stop_directive", "alarm_add_directive"]
        );
        let alarm = added(&outcome, 1);
        assert_eq!(alarm.local_time.map(|t| (t.hours, t.minutes)), Some((7, 19)));
        assert_eq!(alarm.activation_days[0].date, Some(pb::CalendarDate { year: 2026, month: 11, day: 2 }));
        assert_eq!(
            alarm.timezone,
            Some(pb::AlarmTimezone { timezone_name: "Asia/Bangkok".into(), offset_seconds: 25200 })
        );
        assert_eq!(alarm.edit_timestamp, 1793578022);
        let quiet = skill.execute(&intent("snooze", json!({})), &station(the_listed())).await.unwrap();
        assert_eq!(quiet.speech.as_deref(), Some("Сейчас будильник не звонит."));
        assert!(quiet.directives.is_empty());
    }

    #[test]
    fn knows_the_wall_clock_of_a_time_zone() {
        let clock = zoned_clock("2026-11-01T10:00:00Z".parse().unwrap(), "Asia/Dubai");
        assert_eq!((clock.hours, clock.minutes, clock.offset_seconds), (14, 0, 14400));
    }

    #[tokio::test]
    async fn stops_ours_while_it_rings_and_cancels_by_time() {
        let skill = skill();
        let now = context("2026-11-01T10:00:00Z", "Asia/Dubai");
        skill.execute(&intent("set", json!({"inSeconds": 120})), &now).await.unwrap();
        let rings: DateTime<Utc> = "2026-11-01T10:02:30Z".parse().unwrap();
        assert!(skill.ringing("h:test", None, rings).is_some());
        assert!(skill.ringing("h:test", None, "2026-11-01T10:30:00Z".parse().unwrap()).is_none());
        let none = skill.execute(&intent("cancel", json!({"hours": 9})), &now).await.unwrap();
        assert_eq!(none.speech.as_deref(), Some("Таких будильников нет."));
        let cancel = skill.execute(&intent("cancel", json!({"hours": 14, "minutes": 2})), &now).await.unwrap();
        assert_eq!(cancel.speech.as_deref(), Some("Отменила будильник."));
        assert!(skill.upcoming("h:test", now.now).is_empty());
    }

    #[tokio::test]
    async fn fails_an_alarm_no_date_holds_as_the_ts_did_and_keeps_nothing() {
        let skill = skill();
        let now = context("2026-11-01T10:00:00Z", "Europe/Moscow");
        // «Разбуди меня через 3000000000 часов», as the alarmIn rule reads it: 342 000 years on.
        let far = skill.execute(&intent("set", json!({"inSeconds": 10_800_000_000_000u64})), &now).await;
        assert_eq!(far.unwrap_err().to_string(), "Invalid time value");
        // A duration saturated where it was read.
        let saturated = skill.execute(&intent("set", json!({"inSeconds": u64::MAX})), &now).await;
        assert_eq!(saturated.unwrap_err().to_string(), "Invalid time value");
        assert!(skill.store.get("h:test").is_empty());
    }

    #[tokio::test]
    async fn sets_an_alarm_whose_local_time_is_past_the_end_of_chronos_range() {
        let skill = skill();
        let now = context("2026-11-01T10:00:00Z", "Europe/Moscow");
        // An hour before chrono's last moment is past it in Moscow; a Date still holds it, and the TS set the alarm.
        let last = DateTime::<Utc>::MAX_UTC.timestamp() - 3600 - now.now.timestamp();
        let edge = skill.execute(&intent("set", json!({"inSeconds": last})), &now).await.unwrap();
        assert_eq!(edge.speech.as_deref(), Some("Будильник на 1:59 завтра."));
    }
}
