//! The turn's line (docs/plans/thinking.md, §10, «The turn's line»): one JSON record of how `OurThinking` answered a
//! request, on the journal's row (`requests.thinking`, D2). No words of the phrase: the row has them, for whoever may
//! read the journal. The numbers every choice of the plan is made on come from here (stats.rs): the card's parts and
//! tokens, each round's times, tokens and calls, the first word, the waits, the shadow's hits, and what came next.
//!
//! `next` is what the person did after the answer, on the same device: `cut` (a `stop`, with how much was heard),
//! `repeated` (the same words again soon), `answered` (a reply to our question), `not_that` (the pack's «не то»),
//! `next` (another phrase), or `quiet` (nothing for a minute). It is written onto the earlier line when the next phrase
//! comes, or after the minute.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::dialog::{RequestParts, Seen, Steer};

/// The line's version.
pub const V: u32 = 1;

fn v() -> u32 {
    V
}

/// The variants a turn ran with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variants {
    pub thinking: String,
    pub card: String,
    pub rounds: String,
    pub delivery: String,
}

/// Who the phrase was said to, how the audio side knew, who answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub by: String,
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// The card of the turn's first round.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Its parts in tokens (estimated): tools, stable, changing, history.
    pub tokens: Tokens,
    /// The groups in its catalog.
    pub groups: u32,
    /// The tools offered.
    pub tools: u32,
    /// The groups picked for the phrase.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked: Vec<String>,
    /// Where the picks came from: `words`, `journal`, `sense`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub tools: u32,
    pub stable: u32,
    pub changing: u32,
    pub history: u32,
}

impl From<RequestParts> for Tokens {
    fn from(parts: RequestParts) -> Tokens {
        Tokens { tools: parts.tools, stable: parts.stable, changing: parts.changing, history: parts.messages }
    }
}

/// One round of the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_ms: Option<u64>,
    #[serde(rename = "in")]
    pub input: u64,
    pub out: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    /// The groups it opened (`open`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opened: Vec<String>,
    /// Its `recall` calls.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub recall: u32,
    /// The tools offered.
    #[serde(default)]
    pub tools: u32,
    /// The last round: words only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub words: bool,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

impl Round {
    pub fn of(seen: &Seen) -> Round {
        let usage = seen.usage.clone().unwrap_or_default();
        let opened = Vec::new();
        Round {
            ms: seen.ms,
            first_ms: seen.first_ms,
            input: usage.input,
            out: usage.output,
            cache_read: usage.cache_read.unwrap_or(0),
            cache_write: usage.cache_write.unwrap_or(0),
            recall: seen.calls.iter().filter(|call| *call == "recall").count() as u32,
            calls: seen.calls.clone(),
            opened,
            tools: seen.parts.offered,
            words: seen.words,
        }
    }
}

/// A `wait` sent: why, the time expected, when (ms since the phrase).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_ms: Option<u64>,
    pub at_ms: u64,
}

/// What the person did next on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Next {
    /// From the end of the answer.
    pub after_ms: u64,
    /// `cut`, `repeated`, `answered`, `not_that`, `next`, `quiet`.
    pub what: String,
    /// How much of a cut answer was heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard_chars: Option<u64>,
}

/// A variant in shadow: what it would have done, against what the live one did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shadow {
    pub part: String,
    pub variant: String,
    #[serde(default)]
    pub tokens: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// `hit`, `miss`, `none` for the card; the policy's own words for the rounds.
    pub hit: String,
    /// The policy's decisions, in shadow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
}

/// An answer of «Чутьё» this turn, as Thinking got it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sensed {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
    pub by: String,
    pub ms: u64,
    /// The pick of a model step in shadow, beside the words' one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<String>,
}

/// The turn's line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    #[serde(default = "v")]
    pub v: u32,
    /// The mark of the settings the turn ran with.
    pub settings: String,
    pub variants: Variants,
    /// The kind of the device, and whether the answer is heard.
    pub kind: String,
    pub voice: bool,
    pub agent: Agent,
    /// The fast route's step that took the phrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rounds: Vec<Round>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_word_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waits: Vec<Wait>,
    pub end: String,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadow: Vec<Shadow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sense: Vec<Sensed>,
}

/// A line as the turn writes it, shared with the rounds' watcher.
#[derive(Clone, Default)]
pub struct Record(Arc<Mutex<Line>>);

impl Record {
    pub fn new(line: Line) -> Record {
        Record(Arc::new(Mutex::new(line)))
    }

    pub fn with(&self, change: impl FnOnce(&mut Line)) {
        change(&mut self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    }

    pub fn line(&self) -> Line {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// A round seen: its numbers, the model, and the card's parts from the first round.
    pub fn seen(&self, seen: &Seen) {
        self.with(|line| {
            if line.rounds.is_empty() {
                let card = line.card.get_or_insert_with(Card::default);
                card.tokens = seen.parts.into();
                card.tools = seen.parts.offered;
            }
            line.provider.get_or_insert_with(|| seen.provider.clone());
            line.model.get_or_insert_with(|| seen.model.clone());
            line.rounds.push(Round::of(seen));
        });
    }

    /// The rounds' watcher that keeps this line.
    pub fn steer(&self) -> Steer {
        let record = self.clone();
        Steer { seen: Some(Arc::new(move |seen: &Seen| record.seen(seen))), ..Steer::default() }
    }
}

/// The name of a door's word, as the line writes it.
pub fn name_of<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default()
}

/// What the next phrase on the device says of the answer before it.
pub fn next_of(
    after_ms: u64,
    phrase: &crate::door::Phrase,
    earlier_text: &str,
    asked: bool,
    repeated_ms: u64,
    words: &super::words::Words,
) -> Next {
    let what = if phrase.context.answer_to.is_some() || (asked && phrase.opened == crate::door::Opened::Window) {
        "answered"
    } else if words.not_that(&phrase.text) {
        "not_that"
    } else if after_ms <= repeated_ms && super::words::plain(&phrase.text) == super::words::plain(earlier_text) {
        "repeated"
    } else {
        "next"
    };
    Next { after_ms, what: what.into(), heard_chars: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_writes_the_plan_s_names_and_reads_back() {
        let line = Line {
            v: V,
            settings: "s-3f9a".into(),
            variants: Variants {
                thinking: "ours".into(),
                card: "all".into(),
                rounds: "today".into(),
                delivery: "append".into(),
            },
            kind: "station".into(),
            voice: true,
            agent: Agent { to: None, by: "default".into(), speaker: Some("alice".into()) },
            rounds: vec![Round {
                ms: 980,
                first_ms: Some(640),
                input: 3900,
                out: 42,
                cache_read: 3600,
                calls: vec!["get_weather".into()],
                tools: 31,
                ..Round::default()
            }],
            first_word_ms: Some(1450),
            budget_ms: Some(1500),
            end: "answered".into(),
            ms: 1800,
            next: Some(Next { after_ms: 8200, what: "quiet".into(), heard_chars: None }),
            ..Line::default()
        };
        let written = serde_json::to_value(&line).unwrap();
        assert_eq!(written["agent"]["as"], "alice");
        assert_eq!(written["rounds"][0]["in"], 3900);
        assert_eq!(written["next"]["what"], "quiet");
        assert!(written.get("fast").is_none() && written.get("shadow").is_none(), "{written}");
        assert_eq!(serde_json::from_value::<Line>(written).unwrap(), line);
        let older: Line = serde_json::from_value(json!({"settings": "s-0000", "variants": {"thinking": "ours", "card": "all", "rounds": "today", "delivery": "append"}, "kind": "computer", "voice": false, "agent": {"by": "chat"}, "end": "answered", "ms": 10, "later": 1})).unwrap();
        assert_eq!(older.v, V);
    }

    fn phrase(text: &str, opened: crate::door::Opened, answer_to: Option<&str>) -> crate::door::Phrase {
        let mut phrase: crate::door::Phrase = serde_json::from_value(json!({
            "turn": "t-2", "text": text, "where": {"device": "kitchen", "kind": "station"}, "opened": "button"
        }))
        .unwrap();
        phrase.opened = opened;
        phrase.context.answer_to = answer_to.map(str::to_string);
        phrase
    }

    #[test]
    fn what_came_next_is_read_from_the_next_phrase() {
        use crate::door::Opened;
        let words = super::super::words::of("ru");
        let next = |text, opened, answer_to, after| {
            next_of(after, &phrase(text, opened, answer_to), "Какая погода?", true, 30_000, words).what
        };
        assert_eq!(next("да", Opened::Window, Some("question"), 2_000), "answered");
        assert_eq!(next("не то, я про завтра", Opened::Button, None, 3_000), "not_that");
        assert_eq!(next("какая погода", Opened::Button, None, 5_000), "repeated");
        assert_eq!(next("какая погода", Opened::Button, None, 50_000), "next", "too late to be a repeat");
        assert_eq!(next("включи радио", Opened::Word, None, 5_000), "next");
    }
}
