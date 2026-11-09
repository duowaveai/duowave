use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// JavaScript's whitespace (`\s`, String.prototype.trim): Unicode spaces and line terminators, U+FEFF included,
/// U+0085 not.
pub(crate) fn is_js_space(c: char) -> bool {
    c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}')
}

/// The same, as a regex class body.
pub(crate) const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

/// What Whisper-family models print on silence or noise: subtitle credits from their training data.
static HALLUCINATIONS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ["^продолжение следует", "^субтитры", "^редактор субтитров", "^спасибо за (просмотр|внимание)", "^подписывайтесь"]
        .into_iter()
        .map(regex)
        .collect()
});

static EDGE_MARKS: LazyLock<Regex> = LazyLock::new(|| regex(r"^[\p{P}\p{S}]+|[\p{P}\p{S}]+$"));
static LETTER_OR_DIGIT: LazyLock<Regex> = LazyLock::new(|| regex(r"[\p{L}\p{N}]"));
static LEADING_MARKS: LazyLock<Regex> = LazyLock::new(|| regex(&format!(r"^[{JS_SPACE}\p{{P}}]+")));
static STARTS_UPPER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\p{Lu}"));
/// "Алиса" as recognition sometimes hears it: «Олеся, потише», «Элиса, …», a whispered «Лариса. Привет».
static ALICE_HEARD_AS: LazyLock<Regex> = LazyLock::new(|| regex(r"^[аоэ]?л[еиія]с[аяеиуь]?$|^ларис[аы]?$"));

fn normalize(word: &str) -> String {
    EDGE_MARKS.replace_all(&word.to_lowercase().replace('ё', "е"), "").into_owned()
}

fn without_last_char(text: &str) -> &str {
    text.char_indices().last().map_or("", |(at, _)| &text[..at])
}

/// "алиса" also matches "алису", "алисе", "алисы"; short words must match exactly.
fn is_wake_word(word: &str, wake_words: &[String]) -> bool {
    wake_words.iter().any(|wake| {
        let length = wake.chars().count();
        word == wake || (length >= 5 && word.chars().count() <= length + 1 && word.starts_with(without_last_char(wake)))
    })
}

/// The first word addressed with a comma or a stop, sounding like "Алиса": the wake word misheard, not a word of the
/// command.
fn is_misheard_alice(word: &str, wake_words: &[String]) -> bool {
    wake_words.iter().any(|wake| wake == "алиса")
        && (word.ends_with(',') || word.ends_with('.'))
        && ALICE_HEARD_AS.is_match(&normalize(word))
}

fn ends_phrase(word: &str) -> bool {
    word.ends_with([',', '.', '!', '?', '…'])
}

/// The wake word again inside the command, addressed: «Алиса, стоп. Алиса, включи кино» came as one request (live,
/// 02.11: said before the answer, it went to the model as a conversation). The user started over, and the command is
/// what follows it; with nothing after it, what came before. Only the word itself, after a stop or a comma and
/// followed by one: «позвони Алисе», «включи песню «Алиса»» stay as they are. None: no repeated wake word.
fn after_repeated_wake(words: &[String], wake: &[String]) -> Option<Vec<String>> {
    for i in (0..words.len()).rev() {
        let addressed = ends_phrase(&words[i]) && (i == 0 || ends_phrase(&words[i - 1]));
        if !addressed || !wake.contains(&normalize(&words[i])) {
            continue;
        }
        let after = &words[i + 1..];
        if after.iter().any(|word| LETTER_OR_DIGIT.is_match(word)) {
            return Some(after.to_vec());
        }
        let mut before = words[..i].to_vec();
        // «Как дела, Алиса?» → «Как дела»: not the comma that led to the wake word.
        if let Some(last) = before.last_mut() {
            *last = last.trim_end_matches([',', ';', ':']).to_string();
        }
        return Some(before);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transcript {
    /// The command.
    pub text: String,
    /// The user started over: the command is cut at a repeated wake word, the audio holds more than it.
    pub restarted: bool,
}

/// The first character in upper case, as `charAt(0).toUpperCase()` did (characters past the BMP stay).
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if u32::from(first) <= 0xffff => first.to_uppercase().chain(chars).collect(),
        _ => text.to_string(),
    }
}

/// Turns raw recognition into the command: drops the wake word (and up to two words before it, as in "слушай,
/// Алиса"), a repeated one with what came before it, and known hallucinations. Recognition hears the wake word
/// because its audio starts at the wake word onset; that keeps the first word of a command said without a pause.
pub fn read_transcript(raw: &str, wake_words: &[String]) -> Transcript {
    let words: Vec<String> = raw.split(is_js_space).filter(|word| !word.is_empty()).map(str::to_string).collect();
    let wake: Vec<String> = wake_words.iter().map(|word| normalize(word)).filter(|word| !word.is_empty()).collect();
    let found = words.iter().take(3).position(|word| is_wake_word(&normalize(word), &wake));
    let index = if found.is_none() && words.len() > 1 && is_misheard_alice(&words[0], &wake) { Some(0) } else { found };
    let said = match index {
        Some(index) => words[index + 1..].to_vec(),
        None => words.clone(),
    };
    let repeated = after_repeated_wake(&said, &wake);
    let restarted = repeated.is_some();
    let command = repeated.unwrap_or(said);
    let mut text = LEADING_MARKS.replace(&command.join(" "), "").into_owned();
    let empty = Transcript { text: String::new(), restarted: false };
    if !LETTER_OR_DIGIT.is_match(&text) {
        return empty;
    }
    let normalized = normalize(&text);
    if HALLUCINATIONS.iter().any(|pattern| pattern.is_match(&normalized)) {
        return empty;
    }
    // Keep the model's casing style: "Алиса, включи свет." → "Включи свет."
    if command.first() != words.first() && words.first().is_some_and(|first| STARTS_UPPER.is_match(first)) {
        text = capitalized(&text);
    }
    Transcript { text, restarted }
}

pub fn clean_transcript(raw: &str, wake_words: &[String]) -> String {
    read_transcript(raw, wake_words).text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wake() -> Vec<String> {
        vec!["алиса".into(), "яндекс".into()]
    }

    #[test]
    fn says_when_the_command_is_cut_at_a_repeated_wake_word_the_audio_holds_more_than_it() {
        assert_eq!(
            read_transcript("Алиса, стоп. Алиса, включи кино.", &wake()),
            Transcript { text: "Включи кино.".into(), restarted: true }
        );
        assert_eq!(
            read_transcript("Алиса, включи кино.", &wake()),
            Transcript { text: "Включи кино.".into(), restarted: false }
        );
    }

    #[test]
    fn cleans_each_raw_transcript_to_its_command() {
        let cases = [
            ("Алиса, привет.", "Привет."),
            ("алиса привет", "привет"),
            ("Слушай, Алиса, включи свет", "Включи свет"),
            ("Яндекс, какая погода?", "Какая погода?"),
            ("алису включи музыку", "включи музыку"),
            ("Олеся, потише.", "Потише."),
            ("Элиса, какая погода?", "Какая погода?"),
            ("Олеся пришла домой", "Олеся пришла домой"),
            // A whisper under the radio, 01.11: the wake word came as «Лариса» and «Ларис», with a stop.
            ("Лариса. Привет. Привет.", "Привет. Привет."),
            ("Ларис, привет, привет.", "Привет, привет."),
            ("Лариса пришла домой", "Лариса пришла домой"),
            ("расскажи про лису, пожалуйста", "расскажи про лису, пожалуйста"),
            ("включи свет", "включи свет"),
            ("скажи мне как зовут тебя алиса", "скажи мне как зовут тебя алиса"),
            // Said again before the answer (live, 02.11): the command is what follows the last «Алиса».
            ("Алиса, стоп. Алиса, включи кино.", "Включи кино."),
            ("Алиса, стоп. Алиса.", "Стоп."),
            ("Алиса, как дела, Алиса?", "Как дела"),
            ("Алиса, позвони Алисе, пожалуйста.", "Позвони Алисе, пожалуйста."),
            ("Алиса, включи песню «Алиса».", "Включи песню «Алиса»."),
        ];
        for (raw, expected) in cases {
            assert_eq!(clean_transcript(raw, &wake()), expected, "{raw}");
        }
    }

    #[test]
    fn returns_nothing_for_the_wake_word_alone_punctuation_and_known_hallucinations() {
        assert_eq!(clean_transcript("Алиса.", &wake()), "");
        assert_eq!(clean_transcript("  ", &wake()), "");
        assert_eq!(clean_transcript("Алиса, ...", &wake()), "");
        assert_eq!(clean_transcript("Продолжение следует...", &wake()), "");
        assert_eq!(clean_transcript("Субтитры сделал DimaTorzok", &wake()), "");
    }

    #[test]
    fn does_not_treat_short_words_that_merely_start_like_a_wake_word_as_one() {
        assert_eq!(clean_transcript("али баба", &["али".to_string()]), "баба");
        assert_eq!(clean_transcript("алиби есть", &wake()), "алиби есть");
    }
}
