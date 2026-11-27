//! Whether a thought is over (docs/plans/audio-interface-2.md, «К1. Мысль»): the panel records a phrase in pieces, cut
//! at pauses, and each piece comes back from «Слух» judged — how long to wait for more before the thought is over. The
//! words that decide it are the household language's lists (words.rs); the numbers are here, in one place, and the
//! panel takes them from the answer. The station's way of sound takes the same judge (the plan's part С), and so does
//! the computer's own ear (crates/duowave-ears/src/channel): «Чутьё»'s kind `finished`, its words step (mod.rs).
//!
//! | Heard | End | Silence to wait after the speech |
//! |---|---|---|
//! | the whole first piece is a short word: «да», «стоп» | `short` | none — over as soon as it is heard |
//! | a plain phrase | `plain` | `PLAIN_MS` |
//! | it ends with «и», «чтобы», «который» | `hanging` | `HANGING_MS` |
//! | it ends with «подожди», «дай подумать» | `wait` | `WAIT_MS` |
//!
//! The words «подожди» are not part of the wish: they are taken off the piece's text, at its end and at its start.

use serde::Serialize;

use super::words::Words;

/// After a plain phrase — the computer's pause before 15.11 too.
pub const PLAIN_MS: u64 = 800;
/// After a word a thought does not end with.
pub const HANGING_MS: u64 = 3_500;
/// After «подожди».
pub const WAIT_MS: u64 = 20_000;
/// A thought at most this long, all its pieces together.
pub const THOUGHT_MAX_MS: u64 = 60_000;
/// A thought left hanging waits this long for the next phrase to finish it.
pub const PARK_MS: u64 = 120_000;

/// How a piece ends its thought.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Short,
    Plain,
    Hanging,
    Wait,
}

/// A piece judged: how it ends the thought, how long to wait for more, the text to add to the thought, and the
/// thought's limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thought {
    pub end: End,
    pub wait_ms: u64,
    /// The piece's text without the words of waiting; empty — nothing to add.
    pub text: String,
    pub max_ms: u64,
    pub park_ms: u64,
}

impl Thought {
    fn new(end: End, text: String) -> Thought {
        let wait_ms = match end {
            End::Short => 0,
            End::Plain => PLAIN_MS,
            End::Hanging => HANGING_MS,
            End::Wait => WAIT_MS,
        };
        Thought { end, wait_ms, text, max_ms: THOUGHT_MAX_MS, park_ms: PARK_MS }
    }
}

/// The text's whitespace tokens from `from` to `to` (not including), joined back.
fn tokens(text: &str, from: usize, to: usize) -> String {
    text.split_whitespace().skip(from).take(to.saturating_sub(from)).collect::<Vec<_>>().join(" ")
}

/// A piece of a thought judged by a language's words. `first` — the thought's first piece: a short word ends the
/// thought only there («да» in the middle of a phrase does not).
pub fn judge(text: &str, words: &Words, first: bool) -> Thought {
    let traced = words.split_traced(text);
    let all: Vec<String> = traced.iter().map(|(word, _)| word.clone()).collect();
    let count = text.split_whitespace().count();
    if all.is_empty() {
        return Thought::new(End::Plain, String::new());
    }
    // The words of waiting at the end: the thought waits; they are not part of it.
    let at_end = Words::tail(words.wait(), &all);
    if at_end > 0 {
        let cut = traced[all.len() - at_end].1;
        let rest = tokens(text, 0, cut);
        let rest = strip_head(&rest, words);
        return Thought::new(End::Wait, rest);
    }
    let text = strip_head(text, words);
    let all = words.split(&text);
    if all.is_empty() {
        return Thought::new(End::Wait, String::new());
    }
    if first && count > 0 && words.is_short(&all) {
        return Thought::new(End::Short, text);
    }
    if Words::tail(words.hanging(), &all) > 0 {
        return Thought::new(End::Hanging, text);
    }
    Thought::new(End::Plain, text)
}

/// The words of waiting at the start taken off: «Подожди, сделай игру» — the person went on at once.
fn strip_head(text: &str, words: &Words) -> String {
    let traced = words.split_traced(text);
    let all: Vec<String> = traced.iter().map(|(word, _)| word.clone()).collect();
    let at_start = Words::head(words.wait(), &all);
    if at_start == 0 {
        return text.trim().to_string();
    }
    // The token after the last word of waiting; a token that holds more words than the waiting is dropped whole.
    let from = traced.get(at_start).map(|(_, token)| *token).unwrap_or_else(|| text.split_whitespace().count());
    let from = if traced.get(at_start - 1).map(|(_, token)| *token) == Some(from) { from + 1 } else { from };
    tokens(text, from, text.split_whitespace().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sense::words;

    fn ends(text: &str, code: &str, first: bool) -> (End, u64, String) {
        let thought = judge(text, words::of(code), first);
        (thought.end, thought.wait_ms, thought.text)
    }

    #[test]
    fn a_thought_that_ends_on_a_joining_word_waits_for_more() {
        assert_eq!(ends("Сделай виджет, чтобы", "ru", true), (End::Hanging, HANGING_MS, "Сделай виджет, чтобы".into()));
        assert_eq!(ends("и ещё кнопку и", "ru", false).0, End::Hanging);
        assert_eq!(ends("я хочу виджет который", "ru", true).0, End::Hanging);
        assert_eq!(ends("потому что", "ru", true).0, End::Hanging, "two words of one entry");
        assert_eq!(
            ends("Сделай виджет с курсом доллара.", "ru", true),
            (End::Plain, PLAIN_MS, "Сделай виджет с курсом доллара.".into())
        );
        assert_eq!(ends("Make a widget that shows the weather and", "en", true).0, End::Hanging);
        assert_eq!(ends("Make a widget for the", "en", true).0, End::Hanging);
        assert_eq!(ends("Make a widget with the weather", "en", true).0, End::Plain);
    }

    #[test]
    fn a_short_word_ends_the_thought_at_once_but_only_as_its_first_piece() {
        assert_eq!(ends("Да.", "ru", true), (End::Short, 0, "Да.".into()));
        assert_eq!(ends("стоп", "ru", true).0, End::Short);
        assert_eq!(ends("на стол", "ru", true).0, End::Short);
        assert_eq!(ends("да", "ru", false).0, End::Plain, "«да» in the middle of a thought does not end it");
        assert_eq!(ends("да сделай погоду", "ru", true).0, End::Plain);
        assert_eq!(ends("Yes", "en", true).0, End::Short);
        assert_eq!(ends("never mind", "en", true).0, End::Short);
    }

    #[test]
    fn the_words_of_waiting_wait_longer_and_stay_out_of_the_wish() {
        assert_eq!(ends("Подожди", "ru", true), (End::Wait, WAIT_MS, String::new()));
        assert_eq!(ends("дай подумать…", "ru", true), (End::Wait, WAIT_MS, String::new()));
        assert_eq!(ends("Сделай игру, подожди", "ru", false), (End::Wait, WAIT_MS, "Сделай игру,".into()));
        assert_eq!(
            ends("Подожди, сделай игру в слова", "ru", true),
            (End::Plain, PLAIN_MS, "сделай игру в слова".into())
        );
        assert_eq!(ends("Hold on", "en", true), (End::Wait, WAIT_MS, String::new()));
        assert_eq!(ends("let me think. make a game", "en", true), (End::Plain, PLAIN_MS, "make a game".into()));
    }

    #[test]
    fn a_language_without_words_judges_every_phrase_plain() {
        assert_eq!(ends("Mach ein Widget, damit", "de", true), (End::Plain, PLAIN_MS, "Mach ein Widget, damit".into()));
        assert_eq!(ends("", "ru", true), (End::Plain, PLAIN_MS, String::new()));
    }
}
