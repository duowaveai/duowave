//! Thinking's kinds for «Чутьё» (docs/plans/thinking.md, §13; the contract's «Чутьё»): `answer`, `work`, `quick`,
//! `groups`, `profile`. «Чутьё» is the audio side's service and Thinking asks it as it is (`Sense::ask`); what
//! Thinking brings is the words steps of its kinds, wrapping today's code, and their state in `State.data`. So the
//! decisions stay today's — the rules' and the words' — and a model step, once «Чутьё» has one, is turned on by the
//! setting `thinking.sense.live` and runs in shadow before.
//!
//! | Kind | The words step | `State.data` | The pick |
//! |---|---|---|---|
//! | `answer` | the language's yes and no words, the choice's options | `{expects}` | `yes`, `no`, an option, `other` |
//! | `work` | the talk layer's rules: the workshop's door, the works' words | `{device}` | `door`, `works` |
//! | `quick` | the router of the skills (its quick rules) | `{device, wake}` | the capability |
//! | `groups` | the groups' words of the language (gateway/locales/thinking) | — | the first group; `p` all of them |
//! | `profile` | `choose_profile` | `{command}` | `fast`, `smart` |

use std::sync::{Arc, Weak};

use serde_json::{json, Value};

use crate::sense::{Answer, By, Kind, Question, Sense, State, Step, WordsStep};
use crate::skills::router::{Decision, RouteInput};
use crate::skills::SkillsService;
use crate::talk::rules::TalkRules;
use crate::talk::TalkLayer;

/// The words steps of Thinking's kinds.
pub struct Words {
    pub layer: Weak<TalkLayer>,
    pub skills: Weak<SkillsService>,
}

fn sure(pick: &str) -> Option<Answer> {
    Some(Answer { pick: Some(pick.to_string()), p: vec![(pick.to_string(), 1.0)], by: By::Words, ms: 0 })
}

fn none() -> Option<Answer> {
    Some(Answer { pick: None, p: Vec::new(), by: By::Words, ms: 0 })
}

impl Step for Words {
    fn by(&self) -> By {
        By::Words
    }

    fn ask(&self, question: &Question) -> Option<Answer> {
        let data = &question.state.data;
        let words = super::words::of(question.lang.as_deref().unwrap_or("ru"));
        match question.kind {
            Kind::Answer => {
                if let Some(options) =
                    data.get("expects").and_then(|expects| expects.get("choice")).and_then(Value::as_array)
                {
                    let said = super::words::plain(&question.text);
                    let picked = options
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|option| said.contains(&super::words::plain(option)));
                    return sure(picked.unwrap_or("other"));
                }
                sure(match words.reply(&question.text) {
                    super::words::Reply::Yes => "yes",
                    super::words::Reply::No => "no",
                    super::words::Reply::Other => "other",
                })
            }
            Kind::Groups => {
                let groups = words.groups_in(&question.text);
                let Some(first) = groups.first().cloned() else { return none() };
                Some(Answer {
                    pick: Some(first),
                    p: groups.into_iter().map(|group| (group, 1.0)).collect(),
                    by: By::Words,
                    ms: 0,
                })
            }
            Kind::Profile => {
                let command = data.get("command").and_then(Value::as_bool).unwrap_or(false);
                sure(match crate::skills::quick_commands::choose_profile(&question.text, command) {
                    crate::config::ModelProfile::Smart => "smart",
                    _ => "fast",
                })
            }
            Kind::Work => {
                let layer = self.layer.upgrade()?;
                let device = data.get("device").and_then(Value::as_str).unwrap_or_default();
                let normalized = crate::skills::text::normalize(&question.text);
                let rules = TalkRules::new(&layer);
                if rules.through_door(&normalized, device).is_some() {
                    return sure("door");
                }
                if rules.hear(&normalized, device).is_some() {
                    return sure("works");
                }
                none()
            }
            Kind::Quick => {
                let skills = self.skills.upgrade()?;
                // A skill's dialog going on there (`{capability, until}`): its next phrase is its.
                let follow = data.get("follow").filter(|follow| !follow.is_null());
                let continuation = follow.and_then(|follow| {
                    Some(crate::skills::router::Continuation {
                        capability: crate::skills::capabilities::is_capability(follow.get("capability")?.as_str()?)?,
                        executor: crate::skills::routes::RouteStep::Own,
                        until: follow.get("until")?.as_u64()?,
                    })
                });
                let input = RouteInput {
                    text: question.text.clone(),
                    device_tag: data.get("device").and_then(Value::as_str).unwrap_or_default().to_string(),
                    wake_word: data.get("wake").and_then(Value::as_bool).unwrap_or(false),
                    continuation,
                    ..RouteInput::default()
                };
                match skills.router.decide(&super::hands::filled(&skills, None, input)) {
                    Decision::Intent { intent, .. } => sure(intent.capability.as_str()),
                    _ => none(),
                }
            }
            _ => None,
        }
    }
}

/// «Чутьё» with Thinking's words steps first, then the audio side's own.
pub fn sense(layer: &Arc<TalkLayer>, skills: &Arc<SkillsService>) -> Sense {
    Sense::new(vec![
        Box::new(Words { layer: Arc::downgrade(layer), skills: Arc::downgrade(skills) }),
        Box::new(WordsStep),
    ])
}

/// A question of one of Thinking's kinds.
pub fn question(kind: Kind, text: &str, lang: Option<&str>, data: Value) -> Question {
    Question {
        kind,
        text: text.to_string(),
        lang: lang.map(str::to_string),
        state: State { data, ..State::default() },
        options: Vec::new(),
        deadline_ms: None,
    }
}

/// A kind's name, as the setting and the line write it.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Answer => "answer",
        Kind::Work => "work",
        Kind::Quick => "quick",
        Kind::Groups => "groups",
        Kind::Profile => "profile",
        Kind::Finished => "finished",
        Kind::ForUs => "for_us",
        Kind::Window => "window",
    }
}

/// An answer as the line keeps it.
pub fn sensed(kind: Kind, answer: &Answer) -> super::line::Sensed {
    super::line::Sensed {
        kind: kind_name(kind).into(),
        pick: answer.pick.clone(),
        by: super::line::name_of(&answer.by),
        ms: answer.ms,
        shadow: None,
    }
}

/// What a yes-or-no question expects, for `State.data`.
pub fn yes_no() -> Value {
    json!({"expects": "yes_no"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(sense: &Sense, kind: Kind, text: &str, data: Value) -> Option<String> {
        sense.ask(&question(kind, text, Some("ru"), data)).pick
    }

    #[test]
    fn the_words_steps_answer_thinking_s_kinds_as_today_s_code() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let sense = Sense::new(vec![
            Box::new(Words { layer: Arc::downgrade(&layer), skills: Weak::new() }),
            Box::new(WordsStep),
        ]);
        assert_eq!(ask(&sense, Kind::Answer, "да, звони", yes_no()).as_deref(), Some("yes"));
        assert_eq!(ask(&sense, Kind::Answer, "нет, не надо", yes_no()).as_deref(), Some("no"));
        let choice = json!({"expects": {"choice": ["на стол", "примерить"]}});
        assert_eq!(ask(&sense, Kind::Answer, "давай на стол", choice.clone()).as_deref(), Some("на стол"));
        assert_eq!(ask(&sense, Kind::Answer, "а который час", choice).as_deref(), Some("other"));
        assert_eq!(ask(&sense, Kind::Groups, "какая погода завтра", Value::Null).as_deref(), Some("weather"));
        assert_eq!(ask(&sense, Kind::Groups, "привет", Value::Null), None);
        let smart = crate::skills::quick_commands::choose_profile("привет", true);
        assert_eq!(smart, crate::config::ModelProfile::Smart);
        assert_eq!(ask(&sense, Kind::Profile, "привет", json!({"command": true})).as_deref(), Some("smart"));
        assert_eq!(ask(&sense, Kind::Profile, "привет", json!({})).as_deref(), Some("fast"));
        assert_eq!(ask(&sense, Kind::Work, "привет", json!({"device": "kitchen"})), None, "nothing of the works");
        assert_eq!(ask(&sense, Kind::Window, "хватит", Value::Null).as_deref(), Some("close"), "the audio side's kind");
    }
}
