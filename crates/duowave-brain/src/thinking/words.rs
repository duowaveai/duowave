//! Thinking's words by language (docs/plans/thinking.md, principle 10: no words in the frame): what says «you got me
//! wrong» (`not_that`, the turn's line's `next`), and the stems that pick a group of tools for a phrase (`groups`, the
//! card's phrase's tools, §4). One file per language, `gateway/locales/thinking/<code>.json`; a language without one
//! takes the Russian file, the household's first language. The home's own additions are the setting's (`groups`).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const FILES: &[(&str, &str)] = &[
    ("ru", include_str!("../../../../gateway/locales/thinking/ru.json")),
    ("en", include_str!("../../../../gateway/locales/thinking/en.json")),
];

/// One language's words.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Words {
    #[serde(default)]
    pub not_that: Vec<String>,
    /// A group's id → the stems of words that pick it.
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,
    /// A yes and a no to our question (a risky hand's «Позвонить маме?»).
    #[serde(default)]
    pub yes: Vec<String>,
    #[serde(default)]
    pub no: Vec<String>,
    /// What the hands say (hands.rs).
    #[serde(default)]
    pub hands: HandsWords,
}

/// What the hands say: to the model (`ask_first`, English or not, it is the model's note) and to people.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HandsWords {
    #[serde(default)]
    pub ask_first: String,
    #[serde(default)]
    pub done: String,
    #[serde(default)]
    pub refused: String,
    #[serde(default)]
    pub no_answer: String,
    #[serde(default)]
    pub no_device: String,
}

/// A reply to a yes-or-no question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Yes,
    No,
    /// Neither: something else.
    Other,
}

static WORDS: LazyLock<Vec<(&'static str, Words)>> = LazyLock::new(|| {
    FILES
        .iter()
        .map(|(code, json)| {
            let words = serde_json::from_str(json).unwrap_or_else(|error| {
                tracing::warn!(target: "thinking", "the words of «{code}» do not read: {error}");
                Words::default()
            });
            (*code, words)
        })
        .collect()
});

/// The words of a language; the household's (Russian) when there is no file for it.
pub fn of(code: &str) -> &'static Words {
    let words = &*WORDS;
    words.iter().find(|(known, _)| *known == code).or_else(|| words.first()).map(|(_, words)| words).expect("a file")
}

/// A phrase as the words are matched against it: lower case, the letter «ё» as «е», no marks.
pub fn plain(text: &str) -> String {
    let lower = text.to_lowercase().replace('ё', "е");
    lower
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Words {
    /// The phrase says we got it wrong: «не то», «я не об этом».
    pub fn not_that(&self, text: &str) -> bool {
        let phrase = plain(text);
        self.not_that.iter().any(|words| {
            let words = plain(words);
            phrase == words || phrase.starts_with(&format!("{words} "))
        })
    }

    /// A reply's yes or no: a no anywhere in it («да нет, не надо» is a no), else a yes it begins with.
    pub fn reply(&self, text: &str) -> Reply {
        let phrase = format!(" {} ", plain(text));
        let has = |word: &String| phrase.contains(&format!(" {} ", plain(word)));
        let begins = |word: &String| phrase.starts_with(&format!(" {} ", plain(word)));
        if self.no.iter().any(has) {
            Reply::No
        } else if self.yes.iter().any(begins) {
            Reply::Yes
        } else {
            Reply::Other
        }
    }

    /// The groups whose stems the phrase has, in the order of the file.
    pub fn groups_in(&self, text: &str) -> Vec<String> {
        let phrase = plain(text);
        self.groups
            .iter()
            .filter(|(_, stems)| stems.iter().any(|stem| phrase.contains(&plain(stem))))
            .map(|(group, _)| group.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_files_read_and_a_language_without_one_takes_the_russian_one() {
        assert!(!of("ru").not_that.is_empty() && !of("en").not_that.is_empty());
        assert_eq!(of("tr").not_that, of("ru").not_that);
        assert!(of("ru").not_that("Не то, я про завтра"));
        assert!(of("ru").not_that("я не об этом"));
        assert!(!of("ru").not_that("Нет, не надо"), "a no is not a «not that»");
        assert!(of("en").not_that("Not that."));
    }

    #[test]
    fn a_reply_is_a_yes_a_no_or_something_else() {
        let ru = of("ru");
        assert_eq!(ru.reply("Да"), Reply::Yes);
        assert_eq!(ru.reply("да, звони"), Reply::Yes);
        assert_eq!(ru.reply("Нет, не надо"), Reply::No);
        assert_eq!(ru.reply("не надо"), Reply::No);
        assert_eq!(ru.reply("а который час?"), Reply::Other);
        assert_eq!(of("en").reply("Yes please"), Reply::Yes);
        assert!(!ru.hands.done.is_empty() && !of("en").hands.no_answer.is_empty());
    }

    #[test]
    fn a_phrase_picks_the_groups_whose_stems_it_has() {
        assert_eq!(of("ru").groups_in("Какая погода завтра?"), ["weather"]);
        assert_eq!(of("ru").groups_in("Что у меня завтра в календаре?"), ["calendar"]);
        assert!(of("ru").groups_in("Привет").is_empty());
        assert_eq!(of("ru").groups_in("Поставь таймер на чай"), ["timers"]);
    }
}
