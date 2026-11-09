//! Our timers: the station keeps and rings them (timer_set_directive, field 15 as Yandex sends it); we keep a copy
//! per device for "how much is left", cancel and stop. Every command is in the shape we saw Yandex send (relay
//! journals, 31.10 and 02.11); cancel stays Yandex's while it is on the route, as it was before.

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::gateway::protocol::protocol_types::DeviceState;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill};
use crate::skills::text::{parse_duration, say_duration};
use crate::skills::typed_directives::{
    new_timer_id, timer_cancel, timer_pause, timer_resume, timer_set, timer_stop, TimerSpec,
};
use crate::storage::StateStore;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTimer {
    pub id: String,
    /// Unix seconds.
    pub start: u64,
    pub seconds: u64,
    /// Seconds left when paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<u64>,
}

/// A timer rings until stopped; after this long without a stop we take it for rung out.
const RINGING_MS: u64 = 10 * 60_000;
/// How long "На сколько поставить таймер?" waits for the answer.
const ASK_MS: u64 = 30_000;

/// When it rings, in ms, wide enough for any duration said: the quick rule takes any number of hours, and the TS
/// counted in floating point. In u64 a long one overflowed: a panic, or a timer wrapped into one that rings at once.
fn due_at(timer: &StoredTimer) -> i128 {
    (i128::from(timer.start) + i128::from(timer.seconds)) * 1000
}

/// A duration as said; one past i64 (a saturated one) as i64's largest rather than wrapped to a negative.
fn say_seconds(seconds: u64) -> String {
    say_duration(i64::try_from(seconds).unwrap_or(i64::MAX), false)
}

pub struct TimersSkill {
    store: StateStore<StoredTimer>,
}

impl TimersSkill {
    pub fn new(store: StateStore<StoredTimer>) -> TimersSkill {
        TimersSkill { store }
    }

    /// Our timers that are due and not stopped; when the station reports its timers, only those it still lists.
    pub fn ringing(&self, device: &str, state: Option<&DeviceState>, now: u64) -> Vec<String> {
        let timers = self.timers(device, now);
        let now = i128::from(now);
        timers
            .into_iter()
            .filter(|timer| {
                timer.paused_at.is_none() && due_at(timer) <= now && now - due_at(timer) < i128::from(RINGING_MS)
            })
            .filter(|timer| state.is_none_or(|state| state.timer_ids.is_empty() || state.timer_ids.contains(&timer.id)))
            .map(|timer| timer.id)
            .collect()
    }

    pub fn knows(&self, device: &str, timer_id: &str) -> bool {
        self.store.get(device).iter().any(|timer| timer.id == timer_id)
    }

    /// Timers still counting down (or paused).
    pub fn active(&self, device: &str, now: u64) -> Vec<StoredTimer> {
        self.timers(device, now)
            .into_iter()
            .filter(|timer| timer.paused_at.is_some() || due_at(timer) > i128::from(now))
            .collect()
    }

    fn run(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let device = context.device_tag.as_str();
        let now = context.now.timestamp_millis().max(0) as u64;
        Ok(match intent.action.as_str() {
            "set" => {
                let seconds = js::to_number(intent.params.get("seconds"));
                if !(seconds.is_finite() && seconds >= 1.0) {
                    return Ok(Outcome {
                        speech: Some("На сколько поставить таймер?".into()),
                        listen: true,
                        continuation: Some(ContinuationRequest { capability: CapabilityName::Timers, ttl_ms: ASK_MS }),
                        ..Outcome::default()
                    });
                }
                self.set(device, js::round(seconds) as u64, now)
            }
            "continue" => {
                let text = js::string_or_empty(intent.params.get("text"));
                match parse_duration(&text).filter(|seconds| *seconds > 0) {
                    Some(seconds) => self.set(device, seconds, now),
                    None => Outcome::speech("Не поняла, на сколько. Скажите, например: таймер на пять минут."),
                }
            }
            "stop" => {
                let ids: Vec<String> = match intent.params.get("timerIds").and_then(|ids| ids.as_array()) {
                    Some(ids) => ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect(),
                    None => self.ringing(device, context.device_state.as_ref(), now),
                };
                self.store
                    .set(device, self.store.get(device).into_iter().filter(|timer| !ids.contains(&timer.id)).collect());
                Outcome::directives(ids.iter().map(|id| timer_stop(id)).collect())
            }
            "cancel" => {
                let timers = self.active(device, now);
                if timers.is_empty() {
                    return Ok(Outcome::speech("Таймеров нет."));
                }
                let cancelled: Vec<&str> = timers.iter().map(|timer| timer.id.as_str()).collect();
                self.store.set(
                    device,
                    self.store
                        .get(device)
                        .into_iter()
                        .filter(|timer| !cancelled.contains(&timer.id.as_str()))
                        .collect(),
                );
                Outcome::said(
                    if timers.len() > 1 {
                        "Отменила таймеры."
                    } else {
                        "Отменила таймер."
                    },
                    timers.iter().map(|timer| timer_cancel(&timer.id)).collect(),
                )
            }
            "pause" => {
                let mut timers: Vec<StoredTimer> =
                    self.active(device, now).into_iter().filter(|timer| timer.paused_at.is_none()).collect();
                if timers.is_empty() {
                    return Ok(Outcome::speech("Таймеров нет."));
                }
                for timer in &mut timers {
                    let left = (due_at(timer) - i128::from(now)) as f64 / 1000.0;
                    timer.paused_at = Some(js::round(left).max(0.0) as u64);
                }
                self.save(device, &timers);
                Outcome::said("Поставила таймер на паузу.", timers.iter().map(|timer| timer_pause(&timer.id)).collect())
            }
            "resume" => {
                let mut timers: Vec<StoredTimer> =
                    self.active(device, now).into_iter().filter(|timer| timer.paused_at.is_some()).collect();
                if timers.is_empty() {
                    return Ok(Outcome::speech("Таймеров на паузе нет."));
                }
                for timer in &mut timers {
                    let elapsed = timer.seconds.saturating_sub(timer.paused_at.unwrap_or(0));
                    timer.start = (js::round(now as f64 / 1000.0) as u64).saturating_sub(elapsed);
                    timer.paused_at = None;
                }
                self.save(device, &timers);
                Outcome::said("Продолжаю.", timers.iter().map(|timer| timer_resume(&timer.id)).collect())
            }
            "status" => {
                let mut timers = self.active(device, now);
                timers.sort_by_key(due_at);
                let Some(timer) = timers.first() else {
                    return Ok(Outcome::speech("Таймер не установлен."));
                };
                let left = timer
                    .paused_at
                    .unwrap_or_else(|| js::round((due_at(timer) - i128::from(now)) as f64 / 1000.0).max(1.0) as u64);
                let lead = if timer.paused_at.is_some() {
                    "Таймер на паузе, осталось"
                } else {
                    "Осталось"
                };
                Outcome::speech(format!("{lead} {}.", say_seconds(left)))
            }
            other => anyhow::bail!("timers: unknown action {other}"),
        })
    }

    fn set(&self, device: &str, seconds: u64, now: u64) -> Outcome {
        let timer =
            StoredTimer { id: new_timer_id(), start: js::round(now as f64 / 1000.0) as u64, seconds, paused_at: None };
        let mut timers = self.timers(device, now);
        timers.push(timer.clone());
        self.store.set(device, timers);
        // As Yandex answers it.
        Outcome::said(
            format!("Хорошо. {}. Отсчёт пошёл.", say_seconds(seconds)),
            vec![timer_set(&TimerSpec { id: timer.id, start: timer.start, seconds })],
        )
    }

    /// Stored timers without those long rung out.
    fn timers(&self, device: &str, now: u64) -> Vec<StoredTimer> {
        self.store
            .get(device)
            .into_iter()
            .filter(|timer| timer.paused_at.is_some() || i128::from(now) - due_at(timer) < i128::from(RINGING_MS))
            .collect()
    }

    fn save(&self, device: &str, changed: &[StoredTimer]) {
        let mut timers: Vec<StoredTimer> = self
            .store
            .get(device)
            .into_iter()
            .filter(|timer| !changed.iter().any(|other| other.id == timer.id))
            .collect();
        timers.extend_from_slice(changed);
        self.store.set(device, timers);
    }
}

impl OwnSkill for TimersSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Timers
    }

    fn verified(&self, intent: &Intent) -> bool {
        if js::truthy(intent.params.get("external")) {
            return false;
        }
        matches!(intent.action.as_str(), "set" | "continue" | "stop" | "status" | "pause" | "resume")
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
    use crate::storage::Db;
    use chrono::{DateTime, Utc};
    use prost::Message;
    use serde_json::{json, Value};
    use std::sync::Arc;

    fn context(now: DateTime<Utc>) -> ExecutionContext {
        let mut context = ExecutionContext::new("h:test", now);
        context.timezone = Some("Asia/Dubai".into());
        context
    }

    fn intent(action: &str, params: Value) -> Intent {
        Intent {
            capability: CapabilityName::Timers,
            action: action.into(),
            params: params.as_object().cloned().unwrap(),
        }
    }

    fn skill() -> TimersSkill {
        TimersSkill::new(StateStore::open(Arc::new(Db::memory()), "timers"))
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn timer_of(outcome: &Outcome) -> pb::TimerSpec {
        pb::TimerSetPayload::decode(outcome.directives[0].typed_payload.clone().unwrap()).unwrap().timer.unwrap()
    }

    #[tokio::test]
    async fn sets_a_timer_the_way_yandex_does_speech_and_timer_set_directive_with_the_timer_in_field_15() {
        let skill = skill();
        let now = at("2026-11-01T10:00:00Z");
        let outcome = skill.execute(&intent("set", json!({"seconds": 60})), &context(now)).await.unwrap();
        assert_eq!(outcome.speech.as_deref(), Some("Хорошо. 1 минута. Отсчёт пошёл."));
        let directive = &outcome.directives[0];
        assert_eq!(directive.name, "timer_set_directive");
        assert_eq!(directive.analytics_type.as_deref(), Some("set_timer_capability_directive"));
        assert_eq!(directive.payload, None);
        let payload = pb::TimerSetPayload::decode(directive.typed_payload.clone().unwrap()).unwrap();
        assert_eq!(payload.name, "set_timer_capability_directive");
        let timer = payload.timer.unwrap();
        assert_eq!((timer.start_timestamp, timer.duration, timer.remaining), (now.timestamp() as u64, 60, 60));
        assert!(regex::Regex::new(r"^[0-9a-f]{8}(-[0-9a-f]{8}){3}$").unwrap().is_match(&timer.timer_id));
    }

    #[tokio::test]
    async fn tells_what_is_left_survives_a_restart_and_stops_the_timer_while_it_rings() {
        let start = at("2026-11-01T10:00:00Z");
        let db = Arc::new(Db::memory());
        let first = TimersSkill::new(StateStore::open(db.clone(), "timers"));
        let set = first.execute(&intent("set", json!({"seconds": 300})), &context(start)).await.unwrap();
        let id = timer_of(&set).timer_id;

        let skill = TimersSkill::new(StateStore::open(db, "timers"));
        let minute_later = start + chrono::Duration::seconds(60);
        let status = skill.execute(&intent("status", json!({})), &context(minute_later)).await.unwrap();
        assert_eq!(status.speech.as_deref(), Some("Осталось 4 минуты."));

        let ringing_at = start + chrono::Duration::seconds(320);
        let ms = ringing_at.timestamp_millis() as u64;
        let listing = |ids: &[&str]| DeviceState {
            timer_ids: ids.iter().map(|id| id.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(skill.ringing("h:test", Some(&listing(&[&id])), ms), vec![id.clone()]);
        // The station no longer lists it: stopped on the device itself.
        assert!(skill.ringing("h:test", Some(&listing(&["other"])), ms).is_empty());
        let mut ringing = context(ringing_at);
        ringing.device_state = Some(listing(&[&id]));
        let stop = skill.execute(&intent("stop", json!({})), &ringing).await.unwrap();
        assert_eq!(stop.directives[0].name, "timer_stop_directive");
        let payload = pb::TimerIdPayload::decode(stop.directives[0].typed_payload.clone().unwrap()).unwrap();
        assert_eq!(payload, pb::TimerIdPayload { name: String::new(), timer_id: id });
        assert!(skill.ringing("h:test", None, ms).is_empty());
    }

    #[tokio::test]
    async fn asks_for_the_duration_and_takes_the_answer_as_a_follow_up() {
        let skill = skill();
        let ask = skill.execute(&intent("set", json!({})), &context(Utc::now())).await.unwrap();
        assert_eq!(ask.speech.as_deref(), Some("На сколько поставить таймер?"));
        assert!(ask.listen);
        assert_eq!(ask.continuation.map(|c| c.capability), Some(CapabilityName::Timers));
        let set =
            skill.execute(&intent("continue", json!({"text": "на десять минут"})), &context(Utc::now())).await.unwrap();
        assert_eq!(set.speech.as_deref(), Some("Хорошо. 10 минут. Отсчёт пошёл."));
    }

    #[tokio::test]
    async fn pauses_resumes_and_cancels_what_counts_down() {
        let skill = skill();
        let start = at("2026-11-01T10:00:00Z");
        skill.execute(&intent("set", json!({"seconds": 300})), &context(start)).await.unwrap();
        let later = start + chrono::Duration::seconds(100);
        let pause = skill.execute(&intent("pause", json!({})), &context(later)).await.unwrap();
        assert_eq!(pause.speech.as_deref(), Some("Поставила таймер на паузу."));
        let paused = skill.execute(&intent("status", json!({})), &context(later + chrono::Duration::hours(1))).await;
        assert_eq!(paused.unwrap().speech.as_deref(), Some("Таймер на паузе, осталось 3 минуты 20 секунд."));
        let resumed_at = later + chrono::Duration::hours(1);
        let resume = skill.execute(&intent("resume", json!({})), &context(resumed_at)).await.unwrap();
        assert_eq!(resume.directives[0].name, "timer_resume_directive");
        let left = skill.execute(&intent("status", json!({})), &context(resumed_at)).await.unwrap();
        assert_eq!(left.speech.as_deref(), Some("Осталось 3 минуты 20 секунд."));
        let cancel = skill.execute(&intent("cancel", json!({})), &context(resumed_at)).await.unwrap();
        assert_eq!(cancel.speech.as_deref(), Some("Отменила таймер."));
        assert!(skill.active("h:test", resumed_at.timestamp_millis() as u64).is_empty());
    }

    #[tokio::test]
    async fn counts_a_timer_of_any_length_said_without_overflow_and_does_not_ring_it() {
        let skill = skill();
        let now = at("2026-11-01T10:00:00Z");
        let ms = now.timestamp_millis() as u64;
        // «Поставь таймер на 10000000000000 часов»: its due time in ms is past u64.
        skill.execute(&intent("set", json!({"seconds": 36_000_000_000_000_000u64})), &context(now)).await.unwrap();
        // «… на 99999999999999999999 часов»: the duration saturates where it is read, past i64.
        let saturated = skill
            .execute(&intent("continue", json!({"text": "на 99999999999999999999 часов"})), &context(now))
            .await
            .unwrap();
        assert_eq!(
            saturated.speech.as_deref(),
            Some("Хорошо. 2562047788015215 часов 30 минут 7 секунд. Отсчёт пошёл.")
        );
        let status = skill.execute(&intent("status", json!({})), &context(now)).await.unwrap();
        assert_eq!(status.speech.as_deref(), Some("Осталось 10000000000000 часов."));
        assert!(skill.ringing("h:test", None, ms).is_empty());
        assert_eq!(skill.active("h:test", ms).len(), 2);
    }
}
