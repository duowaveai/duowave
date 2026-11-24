//! Phrases for speech out of the text a model streams. Lengths count characters (the Node gateway counted UTF-16
//! units: the same for Russian text).

use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhraseSplitterOptions {
    /// The first phrase may be this short: the station starts speaking as soon as it is synthesized.
    pub first_min_chars: usize,
    /// Later phrases are merged up to at least this length: fewer synthesis requests and smoother speech.
    pub min_chars: usize,
    /// A sentence longer than this is cut at a clause or a space, so a long monologue still streams.
    pub max_chars: usize,
}

impl Default for PhraseSplitterOptions {
    fn default() -> Self {
        PhraseSplitterOptions { first_min_chars: 12, min_chars: 40, max_chars: 220 }
    }
}

/// JavaScript's `\s`: what the Node gateway's patterns took for a space.
const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}";

fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

static LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?m)^[{JS_SPACE}]*(?:[-*•]|[0-9]+[.)])[{JS_SPACE}]+")).unwrap());
static MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_`#~]+").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{JS_SPACE}]+")).unwrap());

/// Markup an LLM may add although the prompt asks it not to; the synthesizer would read it aloud.
pub fn clean_for_speech(text: &str) -> String {
    let text = LIST_MARKER.replace_all(text, "");
    let text = MARKUP.replace_all(&text, "");
    let text = SPACES.replace_all(&text, " ");
    text.trim_matches(is_js_space).to_string()
}

/// The ends of sentences in `text`, in order: punctuation (`.!?…`, then closing quotes and brackets) followed by a
/// space, or line breaks. "3.5" and "т.е." do not end one. (The Node gateway's `/[.!?…]+["»)]*(?=\s)|\n+/` without the
/// look-ahead the regex crate lacks.)
fn sentence_ends(text: &[char]) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let c = text[at];
        if matches!(c, '.' | '!' | '?' | '…') {
            let mut end = at;
            while end < text.len() && matches!(text[end], '.' | '!' | '?' | '…') {
                end += 1;
            }
            while end < text.len() && matches!(text[end], '"' | '»' | ')') {
                end += 1;
            }
            if end < text.len() && is_js_space(text[end]) {
                ends.push(end);
            }
            // No sentence ends inside a run that is not followed by a space.
            at = end;
        } else if c == '\n' {
            let mut end = at;
            while end < text.len() && text[end] == '\n' {
                end += 1;
            }
            ends.push(end);
            at = end;
        } else {
            at += 1;
        }
    }
    ends
}

/// The last clause boundary (`,;:—–` followed by a space) inside `text`.
fn last_clause_end(text: &[char]) -> Option<usize> {
    (0..text.len().saturating_sub(1))
        .rev()
        .find(|&at| matches!(text[at], ',' | ';' | ':' | '—' | '–') && is_js_space(text[at + 1]))
}

/// Cuts the text an LLM streams into phrases for speech synthesis: the first goes out as soon as it is a
/// sentence, so the answer starts sounding while the model is still writing; later ones are whole
/// sentences merged while short.
#[derive(Debug, Clone, Default)]
pub struct PhraseSplitter {
    buffer: Vec<char>,
    emitted: usize,
    options: PhraseSplitterOptions,
}

impl PhraseSplitter {
    pub fn new() -> PhraseSplitter {
        PhraseSplitter::default()
    }

    pub fn with_options(options: PhraseSplitterOptions) -> PhraseSplitter {
        PhraseSplitter { buffer: Vec::new(), emitted: 0, options }
    }

    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buffer.extend(delta.chars());
        let mut phrases = Vec::new();
        while let Some(cut) = self.find_cut() {
            self.take(cut, &mut phrases);
        }
        phrases
    }

    /// What is left when the text is complete.
    pub fn flush(&mut self) -> Vec<String> {
        let mut phrases = Vec::new();
        self.take(self.buffer.len(), &mut phrases);
        phrases
    }

    fn find_cut(&self) -> Option<usize> {
        let min = if self.emitted == 0 { self.options.first_min_chars } else { self.options.min_chars };
        if let Some(end) = sentence_ends(&self.buffer).into_iter().find(|&end| end >= min) {
            return Some(end);
        }
        let max = self.options.max_chars;
        if self.buffer.len() <= max {
            return None;
        }
        // A long sentence: the last clause boundary, or else the last space, within the limit.
        let head = &self.buffer[..max];
        if let Some(clause) = last_clause_end(head) {
            if clause + 1 >= min {
                return Some(clause + 1);
            }
        }
        match head.iter().rposition(|&c| c == ' ') {
            Some(space) if space >= min => Some(space),
            _ => Some(max),
        }
    }

    fn take(&mut self, length: usize, phrases: &mut Vec<String>) {
        let taken: String = self.buffer.drain(..length.min(self.buffer.len())).collect();
        let phrase = clean_for_speech(&taken);
        if phrase.is_empty() {
            return;
        }
        phrases.push(phrase);
        self.emitted += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Streams the text in small deltas, as an LLM does, and collects the phrases.
    fn split(text: &str, delta: usize) -> Vec<String> {
        let mut splitter = PhraseSplitter::new();
        let chars: Vec<char> = text.chars().collect();
        let mut phrases = Vec::new();
        for piece in chars.chunks(delta) {
            phrases.extend(splitter.push(&piece.iter().collect::<String>()));
        }
        phrases.extend(splitter.flush());
        phrases
    }

    #[test]
    fn sends_the_first_sentence_at_once_and_merges_short_later_ones() {
        assert_eq!(
            split("Сейчас половина восьмого. Ещё рано. Можно поспать. А можно сварить кофе и выйти на прогулку.", 3),
            vec!["Сейчас половина восьмого.", "Ещё рано. Можно поспать. А можно сварить кофе и выйти на прогулку."]
        );
    }

    #[test]
    fn does_not_end_a_sentence_inside_a_number() {
        assert_eq!(
            split("Курс сегодня 3.5 процента годовых, а завтра обещают 4.", 3),
            vec!["Курс сегодня 3.5 процента годовых, а завтра обещают 4."]
        );
    }

    #[test]
    fn keeps_a_very_short_first_sentence_together_with_the_next_one() {
        assert_eq!(split("Да. Конечно, включаю.", 3), vec!["Да. Конечно, включаю."]);
    }

    #[test]
    fn cuts_a_long_sentence_at_a_clause_so_it_still_streams() {
        let long = format!("{}длинное начало, {}без точки", "очень ".repeat(30), "и продолжение ".repeat(10));
        let phrases = split(&long, 7);
        assert!(phrases.len() > 1);
        assert!(phrases[0].ends_with("начало,"), "{:?}", phrases[0]);
        assert_eq!(phrases.join(" "), clean_for_speech(&long));
    }

    #[test]
    fn removes_markup_the_synthesizer_would_read_aloud() {
        assert_eq!(clean_for_speech("**Важно:**\n- первое\n- второе\n1. третье"), "Важно: первое второе третье");
    }
}
