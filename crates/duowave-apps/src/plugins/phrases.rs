//! A phrase a plugin's intent takes, the way the built-in quick rules take theirs: the whole command, normalized
//! (lower case, ё as е, no punctuation), with slots — `включи радио {station}` — catching the words in between.
//!
//! A phrase starts with a word of its own: a slot first would take any command that ends in the right words. Two
//! slots never stand side by side: there would be no telling where one ends.
use regex::Regex;
use serde_json::{Map, Value};

use crate::skills::text::normalize;

/// How long a phrase may be, and how many slots it may have.
const MAX_LENGTH: usize = 160;
const MAX_SLOTS: usize = 4;
/// The input key under which an intent's function gets the request itself (its text, the device): no slot's name.
pub const REQUEST: &str = "request";

#[derive(Debug, Clone)]
pub struct Phrase {
    /// As the manifest wrote it: for people.
    pub template: String,
    pattern: Regex,
    slots: Vec<String>,
}

enum Part {
    Words(String),
    Slot(String),
}

fn slot_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Phrase {
    /// The phrase, or why it cannot be one (said in the manifest's check).
    pub fn parse(template: &str) -> Result<Phrase, String> {
        if template.chars().count() > MAX_LENGTH {
            return Err(crate::tr!("фраза «{template}» длиннее {max} знаков", template = template, max = MAX_LENGTH));
        }
        let mut parts = Vec::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            let words = normalize(&rest[..open]);
            if !words.is_empty() {
                parts.push(Part::Words(words));
            }
            let after = &rest[open + 1..];
            let close = after
                .find('}')
                .ok_or_else(|| crate::tr!("фраза «{template}»: нет закрывающей «}»", template = template))?;
            let name = after[..close].trim();
            if !slot_name(name) {
                return Err(crate::tr!(
                    "фраза «{template}»: «{slot}» — имя из латинских букв, цифр и _",
                    template = template,
                    slot = format!("{{{name}}}")
                ));
            }
            if name == REQUEST {
                return Err(crate::tr!(
                    "фраза «{template}»: имя «{name}» занято: под ним функция получает сам запрос",
                    template = template,
                    name = REQUEST
                ));
            }
            if matches!(parts.last(), Some(Part::Slot(_))) {
                return Err(crate::tr!(
                    "фраза «{template}»: два места подряд, между ними нужно слово",
                    template = template
                ));
            }
            parts.push(Part::Slot(name.to_string()));
            rest = &after[close + 1..];
        }
        if rest.contains('}') {
            return Err(crate::tr!("фраза «{template}»: лишняя «}»", template = template));
        }
        let words = normalize(rest);
        if !words.is_empty() {
            parts.push(Part::Words(words));
        }
        if !matches!(parts.first(), Some(Part::Words(_))) {
            return Err(crate::tr!("фраза «{template}» должна начинаться со слова", template = template));
        }
        let slots: Vec<String> = parts
            .iter()
            .filter_map(|part| match part {
                Part::Slot(name) => Some(name.clone()),
                Part::Words(_) => None,
            })
            .collect();
        if slots.len() > MAX_SLOTS {
            return Err(crate::tr!("фраза «{template}»: больше {max} мест", template = template, max = MAX_SLOTS));
        }
        if (1..slots.len()).any(|i| slots[..i].contains(&slots[i])) {
            return Err(crate::tr!("фраза «{template}»: одно имя места дважды", template = template));
        }
        let last = parts.len() - 1;
        let pattern: Vec<String> = parts
            .iter()
            .enumerate()
            .map(|(index, part)| match part {
                Part::Words(words) => regex::escape(words),
                // The last slot takes the rest; one in the middle stops at the first words that follow it.
                Part::Slot(name) if index == last => format!("(?P<{name}>.+)"),
                Part::Slot(name) => format!("(?P<{name}>.+?)"),
            })
            .collect();
        let pattern = Regex::new(&format!("^{}$", pattern.join(" ")))
            .map_err(|error| crate::tr!("фраза «{template}»: {error}", template = template, error = error))?;
        Ok(Phrase { template: template.to_string(), pattern, slots })
    }

    /// The slots' words when the phrase takes the whole of this normalized text.
    pub fn matches(&self, normalized: &str) -> Option<Map<String, Value>> {
        let found = self.pattern.captures(normalized)?;
        let mut slots = Map::new();
        for name in &self.slots {
            let value = found.name(name)?.as_str().trim();
            if value.is_empty() {
                return None;
            }
            slots.insert(name.clone(), Value::String(value.to_string()));
        }
        Some(slots)
    }

    pub fn slots(&self) -> &[String] {
        &self.slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn heard(phrase: &Phrase, text: &str) -> Option<Value> {
        phrase.matches(&normalize(text)).map(Value::Object)
    }

    #[test]
    fn takes_the_whole_command_and_catches_the_slots_words() {
        let radio = Phrase::parse("Включи радио {station}").unwrap();
        assert_eq!(heard(&radio, "Алиса, включи радио Джаз ФМ!"), Some(json!({"station": "джаз фм"})));
        assert_eq!(heard(&radio, "включи радио"), None, "a slot needs words");
        assert_eq!(heard(&radio, "пожалуйста включи радио рок"), Some(json!({"station": "рок"})));
        assert_eq!(heard(&radio, "включи музыку"), None);
        assert_eq!(heard(&radio, "а включи радио рок"), None, "anchored at the start");

        let route = Phrase::parse("когда автобус {number} до {stop}").unwrap();
        assert_eq!(
            heard(&route, "Когда автобус 25 до Речного вокзала?"),
            Some(json!({"number": "25", "stop": "речного вокзала"}))
        );
        assert_eq!(route.slots(), ["number", "stop"]);
        let plain = Phrase::parse("какие радиостанции есть").unwrap();
        assert_eq!(heard(&plain, "Какие радиостанции есть?"), Some(json!({})));
    }

    #[test]
    fn refuses_a_phrase_that_would_take_too_much() {
        for (template, problem) in [
            ("{station}", "начинаться со слова"),
            ("{a} {b}", "два места подряд"),
            ("включи {request}", "занято"),
            ("включи {a} {b}", "два места подряд"),
            ("включи {a", "закрывающей"),
            ("включи a}", "лишняя"),
            ("включи {станция}", "латинских"),
            ("включи {a} и {a}", "дважды"),
            ("", "начинаться со слова"),
        ] {
            let error = Phrase::parse(template).unwrap_err();
            assert!(error.contains(problem), "{template}: {error}");
        }
    }
}
