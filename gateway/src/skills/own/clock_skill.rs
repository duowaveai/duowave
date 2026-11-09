//! Time and date where the station is, without the model: the most frequent question, answered at once.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, Offset, Timelike, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;

use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

/// Weekdays as Russian dates name them, Monday first.
const WEEKDAYS: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];
/// Months in the genitive: «2 ноября».
const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

/// The wall clock in a time zone; the server's own for none or one the gateway does not know. None where the local
/// time is past an end of chrono's range, as it is within hours of one in a zone off UTC (`naive_local()` panics).
pub fn wall_clock(now: DateTime<Utc>, timezone: Option<&str>) -> Option<NaiveDateTime> {
    let offset = match timezone.and_then(|zone| zone.parse::<Tz>().ok()) {
        Some(zone) => now.with_timezone(&zone).offset().fix(),
        None => now.with_timezone(&Local).offset().fix(),
    };
    now.naive_utc().checked_add_offset(offset)
}

/// «16:35», hours in two digits, as ru-RU writes `{hour: '2-digit', minute: '2-digit'}`.
pub fn say_time(clock: &NaiveDateTime) -> String {
    format!("{:02}:{:02}", clock.hour(), clock.minute())
}

/// «2 ноября».
pub fn say_day_month(clock: &NaiveDateTime) -> String {
    format!("{} {}", clock.day(), MONTHS[clock.month0() as usize])
}

/// «понедельник, 2 ноября».
pub fn say_date(clock: &NaiveDateTime) -> String {
    format!("{}, {}", WEEKDAYS[clock.weekday().num_days_from_monday() as usize], say_day_month(clock))
}

/// «2 ноября в 09:07»: a day and a time, as ru-RU writes them together.
pub fn say_day_and_time(clock: &NaiveDateTime) -> String {
    format!("{} в {}", say_day_month(clock), say_time(clock))
}

pub struct ClockSkill;

impl OwnSkill for ClockSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Chat
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let Some(clock) = wall_clock(context.now, context.timezone.as_deref()) else {
                anyhow::bail!("Invalid time value");
            };
            match intent.action.as_str() {
                // As Yandex says it: "16:35."
                "time" => Ok(Outcome::speech(format!("{}.", say_time(&clock)))),
                "date" => Ok(Outcome::speech(format!("Сегодня {}.", say_date(&clock)))),
                other => anyhow::bail!("clock: unknown action {other}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn says_the_time_and_the_date_where_the_station_is() {
        let mut context = ExecutionContext::new("h:test", "2026-11-02T05:07:00Z".parse().unwrap());
        context.timezone = Some("Asia/Dubai".into());
        let time = ClockSkill.execute(&Intent::new(CapabilityName::Chat, "time"), &context).await.unwrap();
        assert_eq!(time.speech.as_deref(), Some("09:07."));
        let date = ClockSkill.execute(&Intent::new(CapabilityName::Chat, "date"), &context).await.unwrap();
        assert_eq!(date.speech.as_deref(), Some("Сегодня понедельник, 2 ноября."));
        assert_eq!(say_day_and_time(&wall_clock(context.now, Some("UTC")).unwrap()), "2 ноября в 05:07");
        context.timezone = Some("Nowhere/City".into());
        assert!(ClockSkill.execute(&Intent::new(CapabilityName::Chat, "time"), &context).await.is_ok());
    }
}
