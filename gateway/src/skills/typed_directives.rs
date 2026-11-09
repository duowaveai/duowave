//! Typed commands (`*_directive`): a serialized proto message in field 15 and no Struct (the station's
//! protobuf-lite build cannot convert one). Timers and alarms have the shapes Yandex sent our station (relay
//! journals, 31.10 and 02.11); do-not-disturb is only half seen (see station-protocol.proto). Fields at their
//! default (0, empty) are left out, as proto3 encoders do.

use bytes::Bytes;
use prost::Message;

use crate::gateway::protocol::pb;
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};

fn typed(name: &str, analytics_type: &str, payload: impl Message) -> Directive {
    Directive {
        r#type: DirectiveType::ClientAction,
        name: name.to_string(),
        analytics_type: Some(analytics_type.to_string()),
        payload: None,
        typed_payload: Some(Bytes::from(payload.encode_to_vec())),
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

/// An id in the shape Yandex gives timers: four groups of eight hex digits.
pub fn new_timer_id() -> String {
    (0..4).map(|_| format!("{:08x}", rand::random::<u32>())).collect::<Vec<_>>().join("-")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerSpec {
    pub id: String,
    /// Unix seconds.
    pub start: u64,
    pub seconds: u64,
}

/// Exactly as Yandex sent it: "set_timer_capability_directive" and the timer, remaining = duration.
pub fn timer_set(timer: &TimerSpec) -> Directive {
    let name = "set_timer_capability_directive";
    let payload = pb::TimerSetPayload {
        name: name.into(),
        timer: Some(pb::TimerSpec {
            timer_id: timer.id.clone(),
            start_timestamp: timer.start,
            duration: timer.seconds,
            remaining: timer.seconds,
            ..Default::default()
        }),
    };
    typed("timer_set_directive", name, payload)
}

/// As Yandex sent it while the timer rang: the id in field 2, no name, no analytics type.
pub fn timer_stop(timer_id: &str) -> Directive {
    typed("timer_stop_directive", "", pb::TimerIdPayload { name: String::new(), timer_id: timer_id.into() })
}

/// As Yandex sent it for «останови таймер»: the name in field 1, the id in field 2, the name as the analytics type.
pub fn timer_cancel(timer_id: &str) -> Directive {
    let name = "cancel_timer_capability_directive";
    typed("timer_cancel_directive", name, pb::TimerIdPayload { name: name.into(), timer_id: timer_id.into() })
}

/// As Yandex sent it for «поставь таймер на паузу»: the name in field 1 and as the analytics type.
pub fn timer_pause(timer_id: &str) -> Directive {
    let name = "pause_timer_capability_directive";
    typed("timer_pause_directive", name, pb::TimerIdPayload { name: name.into(), timer_id: timer_id.into() })
}

/// As Yandex sent it for «продолжи таймер».
pub fn timer_resume(timer_id: &str) -> Directive {
    let name = "resume_timer_capability_directive";
    typed("timer_resume_directive", name, pb::TimerIdPayload { name: name.into(), timer_id: timer_id.into() })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarDate {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlarmSpec {
    pub id: String,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub timezone: String,
    pub offset_seconds: i32,
    /// A one-time alarm on this date; weekdays (1 Monday … 7 Sunday) for a repeating one (not yet seen from Yandex).
    pub date: Option<CalendarDate>,
    pub weekdays: Vec<i32>,
    /// Unix seconds; Yandex leaves it out of a new alarm and copies the original's into a snoozed one.
    pub edit_timestamp: Option<u64>,
}

/// As Yandex sent it for «поставь будильник через 2 минуты»: the name in field 1, the alarm in field 2.
pub fn alarm_add(alarm: &AlarmSpec) -> Directive {
    let name = "alarm_add_directive";
    let activation_days = match alarm.date {
        Some(date) => vec![pb::ActivationDay {
            weekday: 0,
            date: Some(pb::CalendarDate { year: date.year, month: date.month, day: date.day }),
        }],
        None => alarm.weekdays.iter().map(|weekday| pb::ActivationDay { weekday: *weekday, date: None }).collect(),
    };
    let payload = pb::AlarmAddPayload {
        name: name.into(),
        alarm: Some(pb::AlarmSpec {
            alarm_id: alarm.id.clone(),
            local_time: Some(pb::LocalTime { hours: alarm.hours, minutes: alarm.minutes, seconds: alarm.seconds }),
            activation_days,
            timezone: Some(pb::AlarmTimezone {
                timezone_name: alarm.timezone.clone(),
                offset_seconds: alarm.offset_seconds,
            }),
            edit_timestamp: alarm.edit_timestamp.unwrap_or(0),
            ..Default::default()
        }),
    };
    typed(name, name, payload)
}

/// As Yandex sent it for «отмени будильник на 8 утра».
pub fn alarm_cancel(alarm_id: &str) -> Directive {
    let name = "alarm_cancel_directive";
    typed(name, name, pb::AlarmIdPayload { name: name.into(), alarm_id: alarm_id.into() })
}

/// As Yandex sent it for «стоп» while an alarm rang: field 2 alone, no name or analytics type.
pub fn alarm_stop() -> Directive {
    typed("alarm_stop_directive", "", pb::AlarmStopPayload { name: String::new(), kind: 1 })
}

/// «Отложи будильник» as Yandex does it: stop the ringing and add a one-time alarm under a new id (whatever id
/// `alarm` has), keeping the original's edit time.
pub fn alarm_snooze(alarm: &AlarmSpec) -> [Directive; 2] {
    let name = "stop_alarm_capability_directive";
    let stop = typed("alarm_stop_directive", name, pb::AlarmStopPayload { name: name.into(), kind: 1 });
    [stop, alarm_add(&AlarmSpec { id: new_timer_id(), ..alarm.clone() })]
}

/// «Выключить» is what Yandex sent (1, the name in field 1000); «включить» is a guess, Yandex sent no command.
pub fn do_not_disturb(on: bool) -> Directive {
    let name = "set_do_not_disturb_state";
    let state = if on { pb::ForceDoNotDisturbState::ForceEnable } else { pb::ForceDoNotDisturbState::ForceDisable };
    let payload = pb::DoNotDisturbPayload { force_do_not_disturb_state: state as i32, name: name.into() };
    typed("set_do_not_disturb_directive", name, payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    // Field 15 of the commands Yandex sent a station in relay, base64 — with made-up ids and time zone in place of
    // the originals (same lengths, so the bytes keep Yandex's layout).
    const ALARM_ADD: &str = "ChNhbGFybV9hZGRfZGlyZWN0aXZlEkwKIzBhMWIyYzNkLTRlNWY2YTdiLThjOWQwZTFmLTJhM2I0YzVkEgYIBxAIGDoqCRIHCOoPEAsYAjoSCgxBc2lhL0Jhbmdrb2sQ8MQB";
    const SNOOZE_STOP: &str = "Ch9zdG9wX2FsYXJtX2NhcGFiaWxpdHlfZGlyZWN0aXZlEAE=";
    const SNOOZE_ADD: &str = "ChNhbGFybV9hZGRfZGlyZWN0aXZlElIKIzFmMmUzZDRjLTViNmE3OTg4LWE3YjZjNWQ0LWUzZjJhMWIwEgYIBxATGAcqCRIHCOoPEAsYAjoSCgxBc2lhL0Jhbmdrb2sQ8MQBWKaon9cG";
    const ALARM_CANCEL: &str = "ChZhbGFybV9jYW5jZWxfZGlyZWN0aXZlEiIyYzRlNmE4Yi0xZDNmNWE3Yy05ZTBiMmQ0Zi02YThjMGUy";
    const ALARM_STOP: &str = "EAE=";
    const TIMER_PAUSE: &str =
        "CiBwYXVzZV90aW1lcl9jYXBhYmlsaXR5X2RpcmVjdGl2ZRIjM2Q1ZjdiOWEtMmM0ZTZhOGItMGQxZjNhNWMtN2U5YjFkM2Y=";
    const TIMER_RESUME: &str =
        "CiFyZXN1bWVfdGltZXJfY2FwYWJpbGl0eV9kaXJlY3RpdmUSIzNkNWY3YjlhLTJjNGU2YThiLTBkMWYzYTVjLTdlOWIxZDNm";
    const TIMER_CANCEL: &str =
        "CiFjYW5jZWxfdGltZXJfY2FwYWJpbGl0eV9kaXJlY3RpdmUSIzNkNWY3YjlhLTJjNGU2YThiLTBkMWYzYTVjLTdlOWIxZDNm";
    const DND_OFF: &str = "CAHCPhhzZXRfZG9fbm90X2Rpc3R1cmJfc3RhdGU=";

    const TIMER: &str = "3d5f7b9a-2c4e6a8b-0d1f3a5c-7e9b1d3f";

    fn field15(directive: &Directive) -> String {
        STANDARD.encode(directive.typed_payload.as_ref().expect("a typed payload"))
    }

    fn bangkok(id: &str, hours: u32, minutes: u32, seconds: u32, edit_timestamp: Option<u64>) -> AlarmSpec {
        AlarmSpec {
            id: id.into(),
            hours,
            minutes,
            seconds,
            timezone: "Asia/Bangkok".into(),
            offset_seconds: 25200,
            date: Some(CalendarDate { year: 2026, month: 11, day: 2 }),
            weekdays: vec![],
            edit_timestamp,
        }
    }

    #[test]
    fn adds_an_alarm_postav_budilnik_cherez_2_minuty() {
        let directive = alarm_add(&bangkok("0a1b2c3d-4e5f6a7b-8c9d0e1f-2a3b4c5d", 7, 8, 58, None));
        assert_eq!(directive.name, "alarm_add_directive");
        assert_eq!(directive.analytics_type.as_deref(), Some("alarm_add_directive"));
        assert_eq!(directive.payload, None);
        assert_eq!(field15(&directive), ALARM_ADD);
    }

    #[test]
    fn snoozes_as_a_stop_and_a_new_one_time_alarm_that_keeps_the_edit_time() {
        let [stop, add] = alarm_snooze(&bangkok("ignored", 7, 19, 7, Some(1793578022)));
        assert_eq!(stop.name, "alarm_stop_directive");
        assert_eq!(stop.analytics_type.as_deref(), Some("stop_alarm_capability_directive"));
        assert_eq!(field15(&stop), SNOOZE_STOP);
        // Yandex's copy had its own new id; ours gets another one, the rest is the same.
        let ours = add.typed_payload.unwrap().to_vec();
        let yandex = STANDARD.decode(SNOOZE_ADD).unwrap();
        assert_eq!(ours.len(), yandex.len());
        let id = regex::bytes::Regex::new(r"[0-9a-f]{8}(-[0-9a-f]{8}){3}").unwrap().find(&ours).unwrap().range();
        let mut with_yandex_id = ours.clone();
        with_yandex_id[id].copy_from_slice(b"1f2e3d4c-5b6a7988-a7b6c5d4-e3f2a1b0");
        assert_eq!(STANDARD.encode(with_yandex_id), SNOOZE_ADD);
    }

    #[test]
    fn cancels_and_stops_an_alarm() {
        assert_eq!(field15(&alarm_cancel("2c4e6a8b-1d3f5a7c-9e0b2d4f-6a8c0e2")), ALARM_CANCEL);
        assert_eq!(field15(&alarm_stop()), ALARM_STOP);
    }

    #[test]
    fn pauses_resumes_and_cancels_a_timer() {
        let pause = timer_pause(TIMER);
        assert_eq!(pause.name, "timer_pause_directive");
        assert_eq!(pause.analytics_type.as_deref(), Some("pause_timer_capability_directive"));
        assert_eq!(field15(&pause), TIMER_PAUSE);
        assert_eq!(field15(&timer_resume(TIMER)), TIMER_RESUME);
        assert_eq!(field15(&timer_cancel(TIMER)), TIMER_CANCEL);
    }

    #[test]
    fn turns_do_not_disturb_off() {
        let off = do_not_disturb(false);
        assert_eq!(off.name, "set_do_not_disturb_directive");
        assert_eq!(off.analytics_type.as_deref(), Some("set_do_not_disturb_state"));
        assert_eq!(field15(&off), DND_OFF);
    }

    #[test]
    fn gives_timers_ids_in_yandex_shape() {
        let id = new_timer_id();
        assert!(regex::Regex::new(r"^[0-9a-f]{8}(-[0-9a-f]{8}){3}$").unwrap().is_match(&id), "{id}");
        assert_ne!(new_timer_id(), id);
    }
}
