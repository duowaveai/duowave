//! The talk's word lists by language (docs/plans/audio-interface-2.md, «Слова — в пакете языка»): the words that end
//! a thought at once («да», «стоп»), that leave it hanging («и», «чтобы»), that ask to wait («подожди»), and that close
//! the window after our question («хватит», «спасибо»). The frame knows none of them.
//!
//! One file per language — `gateway/locales/talk/<code>.json` — and one line in `BUILT_IN`: adding a language, or a
//! word to a list, touches no code of the frame. The household's language (the setting «Язык», `locale`) picks the
//! file; a language without one has no words, and the frame still works: every phrase is a plain one. No language set
//! — the gateway's own, Russian.
//!
//! A file: `fold` — letters taken as others before comparing («ё» → «е»); `thought.short` — whole phrases;
//! `thought.hanging` — what a thought may end with and not be over; `thought.wait` — «give me a moment», at the end or
//! the start of a piece; `window.close` — whole phrases. Every entry is compared word by word after the same
//! normalization as the heard text: lower case, the folds, no punctuation.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// The files built in, by language code.
const BUILT_IN: &[(&str, &str)] = &[
    ("ru", include_str!("../../../../gateway/locales/talk/ru.json")),
    ("en", include_str!("../../../../gateway/locales/talk/en.json")),
];

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    fold: HashMap<String, String>,
    #[serde(default)]
    thought: ThoughtFile,
    #[serde(default)]
    window: WindowFile,
}

#[derive(Debug, Default, Deserialize)]
struct ThoughtFile {
    #[serde(default)]
    short: Vec<String>,
    #[serde(default)]
    hanging: Vec<String>,
    #[serde(default)]
    wait: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WindowFile {
    #[serde(default)]
    close: Vec<String>,
}

/// One language's lists, each entry as its words.
#[derive(Debug, Default)]
pub struct Words {
    fold: Vec<(String, String)>,
    short: Vec<Vec<String>>,
    hanging: Vec<Vec<String>>,
    wait: Vec<Vec<String>>,
    close: Vec<Vec<String>>,
}

impl Words {
    /// A language's file read: an entry that normalizes to nothing is left out.
    fn read(json: &str) -> Result<Words, serde_json::Error> {
        let file: File = serde_json::from_str(json)?;
        let mut fold: Vec<(String, String)> = file.fold.into_iter().collect();
        // Longer folds first: «’s» before «’».
        fold.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()).then(a.0.cmp(&b.0)));
        let mut words = Words { fold, ..Words::default() };
        let entries = |words: &Words, list: Vec<String>| -> Vec<Vec<String>> {
            list.iter().map(|entry| words.split(entry)).filter(|entry| !entry.is_empty()).collect()
        };
        words.short = entries(&words, file.thought.short);
        words.hanging = entries(&words, file.thought.hanging);
        words.wait = entries(&words, file.thought.wait);
        words.close = entries(&words, file.window.close);
        Ok(words)
    }

    /// One heard word as the lists compare it: lower case, folded, letters and digits only (it may fall into two:
    /// «that's» → «that», «s»).
    fn normal(&self, token: &str) -> Vec<String> {
        let mut text = token.to_lowercase();
        for (from, to) in &self.fold {
            text = text.replace(from.as_str(), to);
        }
        text.chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    /// The words of a text as the lists compare them.
    pub fn split(&self, text: &str) -> Vec<String> {
        text.split_whitespace().flat_map(|token| self.normal(token)).collect()
    }

    /// The words of a text, each with the index of the text's whitespace token it came from.
    pub(super) fn split_traced(&self, text: &str) -> Vec<(String, usize)> {
        text.split_whitespace()
            .enumerate()
            .flat_map(|(at, token)| self.normal(token).into_iter().map(move |word| (word, at)))
            .collect()
    }

    /// The longest entry of a list the words end with: how many words it has; 0 — none.
    pub(super) fn tail(list: &[Vec<String>], words: &[String]) -> usize {
        list.iter().filter(|entry| words.ends_with(entry)).map(Vec::len).max().unwrap_or(0)
    }

    /// The longest entry of a list the words begin with: how many words it has; 0 — none.
    pub(super) fn head(list: &[Vec<String>], words: &[String]) -> usize {
        list.iter().filter(|entry| words.starts_with(entry)).map(Vec::len).max().unwrap_or(0)
    }

    /// Whether the words are one of the list's entries, whole.
    fn whole(list: &[Vec<String>], words: &[String]) -> bool {
        !words.is_empty() && list.iter().any(|entry| entry.as_slice() == words)
    }

    /// The whole phrase is a short one: it ends at once.
    pub fn is_short(&self, words: &[String]) -> bool {
        Words::whole(&self.short, words)
    }

    pub(super) fn hanging(&self) -> &[Vec<String>] {
        &self.hanging
    }

    pub(super) fn wait(&self) -> &[Vec<String>] {
        &self.wait
    }

    /// The whole phrase closes the window after our question: «хватит», «спасибо».
    pub fn closes(&self, text: &str) -> bool {
        Words::whole(&self.close, &self.split(text))
    }

    /// Whether the language has any words at all.
    pub fn is_empty(&self) -> bool {
        self.short.is_empty() && self.hanging.is_empty() && self.wait.is_empty() && self.close.is_empty()
    }
}

static LANGUAGES: LazyLock<HashMap<&'static str, Words>> = LazyLock::new(|| {
    BUILT_IN
        .iter()
        .filter_map(|(code, json)| match Words::read(json) {
            Ok(words) => Some((*code, words)),
            Err(error) => {
                tracing::error!(target: "talk", "the talk's words of {code} do not read: {error}");
                None
            }
        })
        .collect()
});

static NONE: LazyLock<Words> = LazyLock::new(Words::default);

/// The words of a language code (`ru`, `en-GB`); a language without a file — none.
pub fn of(code: &str) -> &'static Words {
    let code = crate::locale::normalize(code).unwrap_or_default();
    LANGUAGES.get(code.as_str()).unwrap_or(&NONE)
}

/// The words of the household's language (the setting «Язык»); none set — the gateway's own language.
pub fn household() -> &'static Words {
    of(&crate::locale::text::household_now().unwrap_or_else(|| crate::locale::SOURCE.to_string()))
}

/// The languages that have words.
pub fn languages() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = LANGUAGES.keys().copied().collect();
    codes.sort_unstable();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_language_reads_and_has_every_list() {
        assert_eq!(languages(), ["en", "ru"], "a file that does not read is left out — and this fails");
        for code in languages() {
            let words = of(code);
            for (name, list) in [
                ("thought.short", &words.short),
                ("thought.hanging", &words.hanging),
                ("thought.wait", &words.wait),
                ("window.close", &words.close),
            ] {
                assert!(!list.is_empty(), "{code}: {name} is empty");
            }
        }
    }

    #[test]
    fn a_language_is_picked_by_its_code_and_one_without_a_file_has_no_words() {
        assert!(of("ru").is_short(&of("ru").split("Да!")));
        assert!(of("en-GB").is_short(&of("en").split("Yes.")));
        assert!(of("de").is_empty(), "no file — no words");
        assert!(!of("de").is_short(&["ja".to_string()]));
        assert!(of("ru").closes("Всё."), "«ё» is folded to «е» on both sides");
        assert!(of("en").closes("That’s all"), "the typographic apostrophe is folded");
        assert!(!of("ru").closes("всё понятно, давай дальше"));
    }

    #[test]
    fn a_new_language_is_a_file_with_its_lists() {
        let words =
            Words::read(r#"{"thought": {"short": ["ja"], "hanging": ["und", "damit"]}, "window": {}}"#).unwrap();
        assert!(words.is_short(&words.split("Ja")));
        assert_eq!(Words::tail(words.hanging(), &words.split("mach ein Widget, damit")), 1);
        assert!(!words.closes("tschüss"), "a list left out is empty");
    }
}
