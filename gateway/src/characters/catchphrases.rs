//! A character's catchphrases: its feed gives them (`phrases` of `catalog.json`: package id → character id →
//! `{ every, lines }`), apart from the package — a feed adds, changes or drops them without a new version, so a pinned
//! archive stays pinned. The gateway says one now and then word for word, before the model's first phrase or after its
//! last (src/dialog/dialog_service.rs, `rounds`): one answer in `every` on average, never two answers running, never
//! the line said last. A line is never told to the model, so a feed can make the character say it, not think it.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::plugins::manifest::valid_id;

/// Lines of one character, at most.
pub const MAX_LINES: usize = 24;
/// Characters in one line, at most.
pub const MAX_CHARS: usize = 120;
/// One answer in this many, when the feed does not say.
pub const DEFAULT_EVERY: u32 = 4;
/// The rarest the feed may ask for.
pub const MAX_EVERY: u32 = 50;

/// Where a line goes in an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseAt {
    /// Before the model's first phrase: a greeting, an exclamation.
    Start,
    /// After its last: a sign-off.
    End,
}

/// A character's catchphrases as its feed gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catchphrases {
    /// One answer in this many has a line, on average.
    pub every: u32,
    pub lines: Vec<(String, PhraseAt)>,
}

/// By package id, then by character id.
pub type FeedPhrases = BTreeMap<String, BTreeMap<String, Catchphrases>>;

/// The `phrases` of a feed: what reads, and what was left out with why. A character with no line that reads is left
/// out; a line too long, empty or with control characters is left out alone.
pub fn parse(value: Option<&Value>) -> (FeedPhrases, Vec<String>) {
    let mut phrases = FeedPhrases::new();
    let mut skipped = Vec::new();
    let Some(value) = value else { return (phrases, skipped) };
    let Some(packages) = value.as_object() else {
        skipped.push(crate::tr!("phrases: нужен объект «пакет → персонаж → фразы»"));
        return (phrases, skipped);
    };
    for (package, characters) in packages {
        if !valid_id(package) {
            skipped.push(crate::tr!("phrases: «{package}» — не id пакета", package = package));
            continue;
        }
        let Some(characters) = characters.as_object() else {
            skipped.push(crate::tr!("phrases.{package}: нужен объект «персонаж → фразы»", package = package));
            continue;
        };
        for (character, given) in characters {
            let at = format!("phrases.{package}.{character}");
            if !valid_id(character) {
                skipped.push(crate::tr!("{at}: не id персонажа", at = at));
                continue;
            }
            match one(given, &at, &mut skipped) {
                Some(read) => {
                    phrases.entry(package.clone()).or_default().insert(character.clone(), read);
                }
                None => skipped.push(crate::tr!("{at}: ни одной фразы", at = at)),
            }
        }
    }
    (phrases, skipped)
}

/// One character's `{ every, lines }`; a line is a text (said at the end) or `{ text, at: "start" | "end" }`.
fn one(value: &Value, at: &str, skipped: &mut Vec<String>) -> Option<Catchphrases> {
    let every = match value.get("every") {
        None => DEFAULT_EVERY,
        Some(every) => match every.as_u64().filter(|every| (1..=u64::from(MAX_EVERY)).contains(every)) {
            Some(every) => every as u32,
            None => {
                skipped.push(crate::tr!("{at}.every: от 1 до {max}", at = at, max = MAX_EVERY));
                DEFAULT_EVERY
            }
        },
    };
    let mut lines = Vec::new();
    for (i, item) in value.get("lines").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let (text, place) = match item {
            Value::String(text) => (Some(text.as_str()), Some("end")),
            Value::Object(_) => {
                (item.get("text").and_then(Value::as_str), item.get("at").and_then(Value::as_str).or(Some("end")))
            }
            _ => (None, None),
        };
        let place = match place {
            Some("start") => Some(PhraseAt::Start),
            Some("end") => Some(PhraseAt::End),
            _ => None,
        };
        let text = text.map(str::trim).filter(|text| {
            !text.is_empty() && text.chars().count() <= MAX_CHARS && !text.chars().any(char::is_control)
        });
        match (text, place) {
            (Some(text), Some(place)) if lines.len() < MAX_LINES => lines.push((text.to_string(), place)),
            (Some(_), Some(_)) => {
                skipped.push(crate::tr!("{at}.lines: не больше {max}", at = at, max = MAX_LINES));
                break;
            }
            _ => skipped.push(crate::tr!(
                "{at}.lines[{i}]: текст до {max} знаков в одну строку, at — start или end",
                at = at,
                i = i,
                max = MAX_CHARS
            )),
        }
    }
    (!lines.is_empty()).then_some(Catchphrases { every, lines })
}

/// Which answer gets a line and which line: one answer in `every` on average, never two answers running, never the
/// line said last while there is another.
#[derive(Debug, Default)]
pub struct Picker {
    last: Option<String>,
    just_said: bool,
}

impl Picker {
    /// A line for this answer, or none. `roll(n)` is a number from 0 to n − 1 (a random one; the tests' own).
    pub fn pick(&mut self, phrases: &Catchphrases, mut roll: impl FnMut(usize) -> usize) -> Option<(String, PhraseAt)> {
        if phrases.lines.is_empty() {
            return None;
        }
        if self.just_said {
            self.just_said = false;
            return None;
        }
        if roll(phrases.every.max(1) as usize) != 0 {
            return None;
        }
        let others: Vec<&(String, PhraseAt)> =
            phrases.lines.iter().filter(|(text, _)| self.last.as_deref() != Some(text.as_str())).collect();
        let pool: Vec<&(String, PhraseAt)> = if others.is_empty() { phrases.lines.iter().collect() } else { others };
        let (text, place) = pool[roll(pool.len()) % pool.len()].clone();
        self.last = Some(text.clone());
        self.just_said = true;
        Some((text, place))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_feeds_phrases_by_package_and_character_and_leaves_out_what_does_not_read() {
        let long = "а".repeat(MAX_CHARS + 1);
        let (phrases, skipped) = parse(Some(&json!({
            "rick-demo": {
                "rick": {
                    "every": 3,
                    "lines": [
                        "Вабба-лабба-даб-даб!",
                        { "text": "Слушай сюда.", "at": "start" },
                        { "text": "  Не усложняй.  " },
                        { "text": "Куда?", "at": "middle" },
                        long,
                        "две\nстроки",
                        42
                    ]
                },
                "nobody": { "lines": [] },
                "Bad Id": { "lines": ["x"] }
            },
            "glados-demo": { "glados": { "every": 0, "lines": ["Ради науки."] } },
            "../evil": { "x": { "lines": ["x"] } }
        })));
        let rick = &phrases["rick-demo"]["rick"];
        assert_eq!(rick.every, 3);
        assert_eq!(
            rick.lines,
            [
                ("Вабба-лабба-даб-даб!".to_string(), PhraseAt::End),
                ("Слушай сюда.".to_string(), PhraseAt::Start),
                ("Не усложняй.".to_string(), PhraseAt::End),
            ]
        );
        assert!(!phrases["rick-demo"].contains_key("nobody"), "a character with no line is left out");
        assert_eq!(phrases["glados-demo"]["glados"].every, DEFAULT_EVERY, "every 0 is not read: the default");
        assert!(!phrases.contains_key("../evil"));
        // middle, too long, two lines, a number; nobody; Bad Id; every 0; ../evil
        assert_eq!(skipped.len(), 8, "{skipped:?}");

        let many: Vec<String> = (0..MAX_LINES + 3).map(|i| format!("Фраза {i}.")).collect();
        let (phrases, skipped) = parse(Some(&json!({ "p": { "c": { "lines": many } } })));
        assert_eq!(phrases["p"]["c"].lines.len(), MAX_LINES);
        assert_eq!(skipped.len(), 1, "{skipped:?}");

        assert!(parse(None).0.is_empty());
        let (phrases, skipped) = parse(Some(&json!(["x"])));
        assert!(phrases.is_empty() && skipped.len() == 1);
    }

    #[test]
    fn a_line_comes_one_answer_in_every_never_twice_running_and_never_the_one_said_last() {
        let phrases = Catchphrases {
            every: 4,
            lines: vec![("Как пожелаете, сэр.".into(), PhraseAt::End), ("К вашим услугам.".into(), PhraseAt::Start)],
        };
        let mut picker = Picker::default();
        // The dice say «not this one»: no line.
        assert_eq!(picker.pick(&phrases, |_| 1), None);
        // They say «this one», the first line.
        assert_eq!(picker.pick(&phrases, |_| 0), Some(("Как пожелаете, сэр.".into(), PhraseAt::End)));
        // The next answer never has one, whatever the dice.
        assert_eq!(picker.pick(&phrases, |_| 0), None);
        // Then the line said last is not taken again while there is another.
        assert_eq!(picker.pick(&phrases, |_| 0), Some(("К вашим услугам.".into(), PhraseAt::Start)));
        assert_eq!(picker.pick(&phrases, |_| 0), None);
        assert_eq!(picker.pick(&phrases, |_| 0), Some(("Как пожелаете, сэр.".into(), PhraseAt::End)));

        // One line alone comes again, never two answers running.
        let one = Catchphrases { every: 1, lines: vec![("Ради науки.".into(), PhraseAt::End)] };
        let mut picker = Picker::default();
        let said: Vec<bool> = (0..4).map(|_| picker.pick(&one, |_| 0).is_some()).collect();
        assert_eq!(said, [true, false, true, false]);

        // The dice are asked for one in `every`.
        let mut asked = Vec::new();
        Picker::default().pick(&phrases, |n| {
            asked.push(n);
            1
        });
        assert_eq!(asked, [4]);
        assert_eq!(Picker::default().pick(&Catchphrases { every: 1, lines: Vec::new() }, |_| 0), None);
    }
}
