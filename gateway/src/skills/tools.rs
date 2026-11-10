//! Capabilities as tools for the model. A call becomes an intent and goes the way a quick rule's intent goes: to
//! our implementation, to Yandex or to "not yet", by the route table.

use std::sync::LazyLock;

use serde_json::{json, Map, Value};

use super::capabilities::{CapabilityName, Intent};
use super::js;
use super::skill::{intent, text_value};
use crate::core::identity::permissions::use_of;
use crate::core::identity::{can, Principal};
use crate::dialog::llm::{LlmTool, ToolCall};

type Input = Map<String, Value>;
/// A call's input as an intent, or why not.
type ToIntent = fn(&Input) -> Result<Intent, String>;
/// The question for Yandex a call asks, if it has one of its own.
type Question = fn(&Input) -> Result<Option<String>, String>;

pub struct ToolSpec {
    pub tool: LlmTool,
    pub capability: CapabilityName,
    to_intent: ToIntent,
    question: Option<Question>,
}

impl ToolSpec {
    /// The call's input as an intent; why not, for input the tool does not take.
    pub fn to_intent(&self, input: &Input) -> Result<Intent, String> {
        (self.to_intent)(input)
    }

    /// The question Yandex gets when it answers the call for the model: built from the call, as the user's own
    /// phrase may ask something else («Что нового?» is about Alice to Yandex, news to the model).
    pub fn question(&self, input: &Input) -> Result<Option<String>, String> {
        match self.question {
            Some(question) => question(input),
            None => Ok(None),
        }
    }
}

fn text(input: &Input, key: &str, required: bool) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => {}
        Some(Value::String(value)) if value.is_empty() => {}
        Some(Value::String(value)) => {
            let trimmed = js::trim(value);
            return Ok((!trimmed.is_empty()).then(|| trimmed.to_string()));
        }
        Some(_) => return Err(format!("{key} must be a string")),
    }
    if required {
        Err(format!("{key} is required"))
    } else {
        Ok(None)
    }
}

/// `text(…, true)` in a template: a value that trims to nothing was undefined there, and said so.
fn required_text(input: &Input, key: &str) -> Result<String, String> {
    text(input, key, true).map(|text| text.unwrap_or_else(|| "undefined".into()))
}

fn integer(input: &Input, key: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    let number = match input.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(text)) => js::string_to_number(text),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    };
    if !number.is_finite() {
        return Err(format!("{key} must be a number"));
    }
    Ok(Some(js::round(number).min(max).max(min)))
}

fn one_of<'a>(input: &Input, key: &str, values: &[&'a str]) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| values.iter().find(|allowed| **allowed == value).copied())
        .ok_or_else(|| format!("{key} must be one of {}", values.join(", ")))
}

fn flag(input: &Input, key: &str) -> Result<bool, String> {
    input.get(key).and_then(Value::as_bool).ok_or_else(|| format!("{key} must be true or false"))
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn number(value: f64) -> Option<Value> {
    Some(js::number_value(value))
}

fn tool(name: &str, description: &str, parameters: Value) -> LlmTool {
    LlmTool { name: name.into(), description: description.into(), parameters }
}

const PLAYBACK: &[&str] = &["stop", "pause", "resume", "next", "previous", "replay", "like", "dislike"];
const TIMER_ACTIONS: &[&str] = &["cancel", "pause", "resume", "status"];
const ALARM_ACTIONS: &[&str] = &["cancel", "status"];
const DAYS: &[&str] = &["today", "tomorrow", "week"];
const HOME_ACTIONS: &[&str] = &["on", "off", "set_brightness", "set_temperature", "scene", "state"];
const CALL_ACTIONS: &[&str] = &["call", "answer", "hangup"];

fn day_words(day: &str) -> &'static str {
    match day {
        "today" => "сегодня",
        "tomorrow" => "завтра",
        _ => "на неделю",
    }
}

/// A clock time «HH:MM» as Number reads its parts.
fn clock_parts(time: &str) -> (f64, f64) {
    let mut parts = time.split(':').map(js::string_to_number);
    (parts.next().unwrap_or(f64::NAN), parts.next().unwrap_or(f64::NAN))
}

pub static TOOLS: LazyLock<Vec<ToolSpec>> = LazyLock::new(|| {
    use CapabilityName::*;
    vec![
        ToolSpec {
            capability: Volume,
            tool: tool(
                "set_volume",
                "Change the speaker volume (0–10). Give level for an absolute value, change for a relative step, or mute.",
                object(
                    json!({
                        "level": {"type": "integer", "minimum": 0, "maximum": 10},
                        "change": {"type": "integer", "minimum": -10, "maximum": 10, "description": "Steps up (positive) or down (negative)."},
                        "mute": {"type": "boolean"},
                    }),
                    &[],
                ),
            ),
            to_intent: |input| {
                if let Some(mute) = input.get("mute").and_then(Value::as_bool) {
                    return Ok(Intent::new(Volume, if mute { "mute" } else { "unmute" }));
                }
                if let Some(level) = integer(input, "level", 0.0, 10.0)? {
                    return Ok(intent(Volume, "set", vec![("level", number(level))]));
                }
                let change = integer(input, "change", -10.0, 10.0)?.unwrap_or(1.0);
                let steps = if change == 0.0 { 1.0 } else { change.abs() };
                Ok(intent(Volume, if change >= 0.0 { "louder" } else { "quieter" }, vec![("steps", number(steps))]))
            },
            question: None,
        },
        ToolSpec {
            capability: Playback,
            tool: tool(
                "control_playback",
                "Stop, pause, resume, skip to the next or the previous track, play the current one again from the start (replay), or like or dislike the current track (a dislike also skips it), of whatever the speaker plays.",
                object(json!({"action": {"type": "string", "enum": PLAYBACK}}), &["action"]),
            ),
            to_intent: |input| Ok(Intent::new(Playback, one_of(input, "action", PLAYBACK)?)),
            question: None,
        },
        ToolSpec {
            capability: Bluetooth,
            tool: tool(
                "bluetooth",
                "Turn Bluetooth pairing of the speaker on or off.",
                object(json!({"on": {"type": "boolean"}}), &["on"]),
            ),
            to_intent: |input| Ok(Intent::new(Bluetooth, if flag(input, "on")? { "on" } else { "off" })),
            question: None,
        },
        ToolSpec {
            capability: Timers,
            tool: tool(
                "set_timer",
                "Start a countdown timer on the speaker. Give it a short name when the person says what it is for \
                 (\"чай\", \"стирка\").",
                object(
                    json!({
                        "seconds": {"type": "integer", "minimum": 1, "maximum": 86400},
                        "name": {"type": "string", "maxLength": 40}
                    }),
                    &["seconds"],
                ),
            ),
            to_intent: |input| {
                let seconds = integer(input, "seconds", 1.0, 86400.0)?;
                let name = input.get("name").and_then(Value::as_str).map(str::trim).filter(|name| !name.is_empty());
                Ok(intent(
                    Timers,
                    "set",
                    vec![("seconds", seconds.and_then(number)), ("name", name.map(|name| Value::from(name.to_string())))],
                ))
            },
            question: None,
        },
        ToolSpec {
            capability: Timers,
            tool: tool(
                "manage_timer",
                "Cancel, pause or resume the running timer, or tell how much time is left.",
                object(json!({"action": {"type": "string", "enum": TIMER_ACTIONS}}), &["action"]),
            ),
            to_intent: |input| Ok(Intent::new(Timers, one_of(input, "action", TIMER_ACTIONS)?)),
            question: None,
        },
        ToolSpec {
            capability: Alarms,
            tool: tool(
                "set_alarm",
                "Set an alarm on the speaker for a clock time (24-hour HH:MM) or in some minutes from now.",
                object(
                    json!({
                        "time": {"type": "string", "pattern": "^\\d{1,2}:\\d{2}$"},
                        "in_minutes": {"type": "integer", "minimum": 1, "maximum": 1440},
                    }),
                    &[],
                ),
            ),
            to_intent: |input| {
                if let Some(time) = text(input, "time", false)? {
                    let (hours, minutes) = clock_parts(&time);
                    if !(hours <= 23.0 && minutes <= 59.0) {
                        return Err("time must be HH:MM".into());
                    }
                    return Ok(intent(Alarms, "set", vec![("hours", number(hours)), ("minutes", number(minutes))]));
                }
                let in_minutes = integer(input, "in_minutes", 1.0, 1440.0)?.ok_or("time or in_minutes is required")?;
                Ok(intent(Alarms, "set", vec![("inSeconds", number(in_minutes * 60.0))]))
            },
            question: None,
        },
        ToolSpec {
            capability: Alarms,
            tool: tool(
                "manage_alarm",
                "Cancel the alarms or tell which alarms are set.",
                object(json!({"action": {"type": "string", "enum": ALARM_ACTIONS}}), &["action"]),
            ),
            to_intent: |input| Ok(Intent::new(Alarms, one_of(input, "action", ALARM_ACTIONS)?)),
            question: None,
        },
        ToolSpec {
            capability: DoNotDisturb,
            tool: tool(
                "do_not_disturb",
                "Turn the do-not-disturb mode of the speaker on or off.",
                object(json!({"on": {"type": "boolean"}}), &["on"]),
            ),
            to_intent: |input| Ok(Intent::new(DoNotDisturb, if flag(input, "on")? { "on" } else { "off" })),
            question: None,
        },
        ToolSpec {
            capability: Radio,
            tool: tool("play_radio", "Play a radio station, by name or any.", object(json!({"station": {"type": "string"}}), &[])),
            to_intent: |input| Ok(intent(Radio, "play", vec![("station", text_value(text(input, "station", false)?.as_deref()))])),
            question: None,
        },
        ToolSpec {
            capability: Music,
            tool: tool(
                "play_music",
                "Play music: an artist, a song, a genre or a mood.",
                object(json!({"query": {"type": "string"}}), &[]),
            ),
            to_intent: |input| Ok(intent(Music, "play", vec![("query", text_value(text(input, "query", false)?.as_deref()))])),
            question: None,
        },
        ToolSpec {
            capability: Tales,
            tool: tool("tell_tale", "Tell or play a fairy tale or an audiobook.", object(json!({"topic": {"type": "string"}}), &[])),
            to_intent: |input| Ok(intent(Tales, "tell", vec![("topic", text_value(text(input, "topic", false)?.as_deref()))])),
            question: None,
        },
        ToolSpec {
            capability: Games,
            tool: tool(
                "start_game",
                "Start a voice game (quiz, words, riddles).",
                object(json!({"game": {"type": "string"}}), &[]),
            ),
            to_intent: |input| Ok(intent(Games, "start", vec![("game", text_value(text(input, "game", false)?.as_deref()))])),
            question: None,
        },
        ToolSpec {
            capability: Reminders,
            tool: tool(
                "set_reminder",
                "Remind the user of something later: in some seconds from now, or at a clock time (24-hour HH:MM) today or on a date.",
                object(
                    json!({
                        "text": {"type": "string", "description": "What to remind about, as it will be said."},
                        "in_seconds": {"type": "integer", "minimum": 1},
                        "at": {"type": "string", "pattern": "^\\d{1,2}:\\d{2}$"},
                        "date": {"type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$"},
                    }),
                    &["text"],
                ),
            ),
            to_intent: |input| {
                let reminder = text(input, "text", true)?;
                let in_seconds = integer(input, "in_seconds", 1.0, 366.0 * 86400.0)?;
                let (at, date) = (text(input, "at", false)?, text(input, "date", false)?);
                Ok(intent(
                    Reminders,
                    "set",
                    vec![
                        ("text", text_value(reminder.as_deref())),
                        ("inSeconds", in_seconds.and_then(number)),
                        ("at", text_value(at.as_deref())),
                        ("date", text_value(date.as_deref())),
                    ],
                ))
            },
            question: None,
        },
        ToolSpec {
            capability: Weather,
            tool: tool(
                "get_weather",
                "Look up the weather forecast; put the result into a short spoken answer.",
                object(json!({"place": {"type": "string"}, "day": {"type": "string", "enum": DAYS}}), &[]),
            ),
            to_intent: |input| {
                let place = text(input, "place", false)?;
                let day = if input.contains_key("day") { Some(one_of(input, "day", DAYS)?) } else { None };
                Ok(intent(Weather, "forecast", vec![("place", text_value(place.as_deref())), ("day", text_value(day))]))
            },
            question: Some(|input| {
                let place = text(input, "place", false)?;
                let day = if input.contains_key("day") { Some(day_words(one_of(input, "day", DAYS)?)) } else { None };
                Ok(Some(match place {
                    Some(place) => ["Погода", place.as_str(), day.unwrap_or("")]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                    None => format!("Какая погода {}", day.unwrap_or("сейчас")),
                }))
            }),
        },
        ToolSpec {
            capability: News,
            tool: tool(
                "get_news",
                "Look up the latest news headlines; retell two or three of them briefly.",
                object(json!({"topic": {"type": "string"}}), &[]),
            ),
            to_intent: |input| Ok(intent(News, "headlines", vec![("topic", text_value(text(input, "topic", false)?.as_deref()))])),
            question: Some(|input| {
                Ok(Some(match text(input, "topic", false)? {
                    Some(topic) => format!("Новости {topic}"),
                    None => "Расскажи новости".into(),
                }))
            }),
        },
        ToolSpec {
            capability: SmartHome,
            tool: tool(
                "smart_home",
                "Control or ask about smart home devices: lights, sockets, climate, scenes.",
                object(
                    json!({
                        "action": {"type": "string", "enum": HOME_ACTIONS},
                        "device": {"type": "string", "description": "The device or room as the user named it."},
                        "value": {"type": "number", "description": "Brightness in percent or temperature in degrees."},
                    }),
                    &["action", "device"],
                ),
            ),
            to_intent: |input| {
                let action = one_of(input, "action", HOME_ACTIONS)?;
                let device = text(input, "device", true)?;
                let value = input.get("value").filter(|value| value.is_number()).cloned();
                Ok(intent(SmartHome, action, vec![("device", text_value(device.as_deref())), ("value", value)]))
            },
            question: Some(|input| {
                let device = required_text(input, "device")?;
                let value = input.get("value").and_then(Value::as_f64).map(|value| js::number_text(js::round(value)));
                Ok(match one_of(input, "action", HOME_ACTIONS)? {
                    "on" => Some(format!("Включи {device}")),
                    "off" => Some(format!("Выключи {device}")),
                    "scene" => Some(format!("Включи сценарий {device}")),
                    "state" => Some(format!("Что с устройством {device}")),
                    "set_brightness" => value.map(|value| format!("Установи яркость {device} {value} процентов")),
                    _ => value.map(|value| format!("Установи температуру {device} {value} градусов")),
                })
            }),
        },
        ToolSpec {
            capability: Calls,
            tool: tool(
                "phone_call",
                "Call a contact, answer the incoming call or hang up.",
                object(json!({"action": {"type": "string", "enum": CALL_ACTIONS}, "contact": {"type": "string"}}), &["action"]),
            ),
            to_intent: |input| {
                let action = one_of(input, "action", CALL_ACTIONS)?;
                Ok(intent(Calls, action, vec![("contact", text_value(text(input, "contact", false)?.as_deref()))]))
            },
            question: None,
        },
        ToolSpec {
            capability: Calls,
            tool: tool(
                "send_voice_message",
                "Send a short message to a contact.",
                object(json!({"contact": {"type": "string"}, "text": {"type": "string"}}), &["contact", "text"]),
            ),
            to_intent: |input| {
                let contact = text(input, "contact", true)?;
                let message = text(input, "text", true)?;
                Ok(intent(
                    Calls,
                    "message",
                    vec![("contact", text_value(contact.as_deref())), ("text", text_value(message.as_deref()))],
                ))
            },
            question: None,
        },
        ToolSpec {
            capability: MusicRecognition,
            tool: tool("recognize_music", "Recognise the song playing around the speaker.", object(json!({}), &[])),
            to_intent: |_| Ok(Intent::new(MusicRecognition, "recognize")),
            question: None,
        },
        ToolSpec {
            capability: MorningShow,
            tool: tool("morning_show", "Start Alice's morning show.", object(json!({}), &[])),
            to_intent: |_| Ok(Intent::new(MorningShow, "start")),
            question: None,
        },
        ToolSpec {
            capability: Skills,
            tool: tool(
                "yandex_skill",
                "Launch a Yandex Dialogs skill (a third-party voice app) by its name.",
                object(json!({"name": {"type": "string"}}), &["name"]),
            ),
            to_intent: |input| Ok(intent(Skills, "launch", vec![("name", text_value(text(input, "name", true)?.as_deref()))])),
            question: Some(|input| Ok(Some(format!("Запусти навык {}", required_text(input, "name")?)))),
        },
        ToolSpec {
            capability: Account,
            tool: tool(
                "yandex_account",
                "Services of the user's Yandex account: the shopping list, finding the phone, purchases.",
                object(json!({"request": {"type": "string"}}), &["request"]),
            ),
            to_intent: |input| {
                Ok(intent(Account, "request", vec![("request", text_value(text(input, "request", true)?.as_deref()))]))
            },
            question: Some(|input| text(input, "request", true)),
        },
    ]
});

/// The station's tools still offered after a text from outside the house: lookups that change nothing.
pub const SAFE_AFTER_OUTSIDE: &[&str] = &["get_weather", "get_news"];

pub fn tool_spec(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|spec| spec.tool.name == name)
}

/// What to ask Yandex for the model's call: the call's own question where the tool has one, else the user's
/// phrase (a reminder, a game: the phrase says it best).
pub fn lookup_question(name: &str, input: &Input, phrase: &str) -> String {
    tool_spec(name).and_then(|spec| spec.question(input).ok().flatten()).unwrap_or_else(|| phrase.to_string())
}

/// The intent of a model's tool call, or why the call cannot be one (said back to the model).
pub fn intent_of(call: &ToolCall) -> Result<Intent, String> {
    if let Some(error) = &call.error {
        return Err(format!("{}: {error}", call.name));
    }
    let spec = tool_spec(&call.name).ok_or_else(|| format!("unknown tool {}", call.name))?;
    spec.to_intent(&call.input).map_err(|error| format!("{}: {error}", call.name))
}

/// Which tools a model's turn is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsKind {
    /// Every tool: the station's commands too.
    Command,
    /// Only those that need no station command.
    Conversation,
}

/// Tools for commands (all of them) or for a conversation (only those that need no station command) — of the
/// capabilities the principal may use: the model is never offered what the person is not allowed.
pub fn tools_for(kind: ToolsKind, principal: Option<&Principal>) -> Vec<LlmTool> {
    TOOLS
        .iter()
        .filter(|spec| kind == ToolsKind::Command || !spec.capability.info().station)
        .filter(|spec| principal.is_none_or(|principal| can(Some(principal), &use_of(spec.capability))))
        .map(|spec| spec.tool.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::Role;

    fn ask(name: &str, input: Value, phrase: &str) -> String {
        lookup_question(name, input.as_object().unwrap(), phrase)
    }

    #[test]
    fn asks_yandex_what_the_call_asks_not_the_user_phrase() {
        let news = "Что нового?";
        assert_eq!(ask("get_news", json!({}), news), "Расскажи новости");
        assert_eq!(ask("get_news", json!({"topic": "спорта"}), news), "Новости спорта");
        assert_eq!(ask("get_weather", json!({}), news), "Какая погода сейчас");
        assert_eq!(ask("get_weather", json!({"day": "tomorrow"}), news), "Какая погода завтра");
        assert_eq!(ask("get_weather", json!({"place": "Москва", "day": "week"}), news), "Погода Москва на неделю");
        assert_eq!(ask("smart_home", json!({"action": "on", "device": "свет на кухне"}), news), "Включи свет на кухне");
        assert_eq!(
            ask("smart_home", json!({"action": "set_brightness", "device": "лампа", "value": 40}), news),
            "Установи яркость лампа 40 процентов"
        );
        assert_eq!(ask("yandex_skill", json!({"name": "Города"}), news), "Запусти навык Города");
        assert_eq!(ask("yandex_account", json!({"request": "что в списке покупок"}), news), "что в списке покупок");
    }

    #[test]
    fn keeps_the_phrase_where_the_call_has_no_question_or_a_wrong_input() {
        assert_eq!(
            ask("set_reminder", json!({"text": "выключить чайник", "in_seconds": 60}), "напомни через минуту"),
            "напомни через минуту"
        );
        assert_eq!(
            ask("smart_home", json!({"action": "set_temperature", "device": "кондиционер"}), "сделай теплее"),
            "сделай теплее"
        );
        assert_eq!(ask("get_weather", json!({"day": "yesterday"}), "какая была погода"), "какая была погода");
        assert_eq!(ask("no_such_tool", json!({}), "привет"), "привет");
    }

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), input: input.as_object().cloned().unwrap(), error: None }
    }

    fn intent_json(name: &str, input: Value) -> Result<Value, String> {
        intent_of(&call(name, input)).map(|intent| serde_json::to_value(intent).unwrap())
    }

    #[test]
    fn makes_intents_of_the_calls_or_says_why_not() {
        assert_eq!(
            intent_json("set_volume", json!({"change": -2.5})),
            Ok(json!({"capability": "volume", "action": "quieter", "params": {"steps": 2}}))
        );
        assert_eq!(
            intent_json("set_volume", json!({"level": "12"})),
            Ok(json!({"capability": "volume", "action": "set", "params": {"level": 10}}))
        );
        assert_eq!(
            intent_json("set_volume", json!({"change": 0})),
            Ok(json!({"capability": "volume", "action": "louder", "params": {"steps": 1}}))
        );
        assert_eq!(
            intent_json("set_volume", json!({"level": "громко"})),
            Err("set_volume: level must be a number".into())
        );
        assert_eq!(
            intent_json("set_alarm", json!({"time": "7:30"})),
            Ok(json!({"capability": "alarms", "action": "set", "params": {"hours": 7, "minutes": 30}}))
        );
        assert_eq!(intent_json("set_alarm", json!({"time": "25:00"})), Err("set_alarm: time must be HH:MM".into()));
        assert_eq!(intent_json("set_alarm", json!({})), Err("set_alarm: time or in_minutes is required".into()));
        assert_eq!(
            intent_json("set_alarm", json!({"in_minutes": 3})),
            Ok(json!({"capability": "alarms", "action": "set", "params": {"inSeconds": 180}}))
        );
        assert_eq!(
            intent_json("control_playback", json!({"action": "fly"})),
            Err("control_playback: action must be one of stop, pause, resume, next, previous, replay, like, dislike"
                .into())
        );
        assert_eq!(intent_json("bluetooth", json!({"on": "yes"})), Err("bluetooth: on must be true or false".into()));
        assert_eq!(
            intent_json("send_voice_message", json!({"contact": "маме"})),
            Err("send_voice_message: text is required".into())
        );
        assert_eq!(
            intent_json("play_radio", json!({"station": "  "})),
            Ok(json!({"capability": "radio", "action": "play", "params": {}}))
        );
        assert_eq!(
            intent_json("play_radio", json!({"station": 5})),
            Err("play_radio: station must be a string".into())
        );
        assert_eq!(
            intent_json("smart_home", json!({"action": "set_brightness", "device": " лампа ", "value": 40})),
            Ok(
                json!({"capability": "smart_home", "action": "set_brightness", "params": {"device": "лампа", "value": 40}})
            )
        );
        assert_eq!(intent_json("no_such_tool", json!({})), Err("unknown tool no_such_tool".into()));
        let broken = ToolCall { error: Some("arguments are not JSON".into()), ..call("set_timer", json!({})) };
        assert_eq!(intent_of(&broken), Err("set_timer: arguments are not JSON".into()));
    }

    #[test]
    fn offers_the_model_only_the_tools_of_what_the_speaker_may_use() {
        let names = |tools: Vec<LlmTool>| tools.into_iter().map(|tool| tool.name).collect::<Vec<_>>();
        let all = names(tools_for(ToolsKind::Command, None));
        let guest = names(tools_for(ToolsKind::Command, Some(&household_principal(Role::Guest, None))));
        assert!(all.contains(&"phone_call".to_string()));
        assert!(!guest.contains(&"phone_call".to_string()));
        assert!(!guest.contains(&"smart_home".to_string()));
        assert!(guest.contains(&"set_timer".to_string()));
        let conversation = names(tools_for(ToolsKind::Conversation, None));
        assert!(conversation.contains(&"get_weather".to_string()) && !conversation.contains(&"set_timer".to_string()));
        assert_eq!(all.len(), 22);
    }
}
