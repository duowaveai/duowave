//! Russian text helpers for the quick rules: normalization, numbers in digits or words, durations and clock times.

use std::sync::LazyLock;

use regex::Regex;

use super::js;

const NUMBERS: &[(&str, f64)] = &[
    ("ноль", 0.0),
    ("один", 1.0),
    ("одна", 1.0),
    ("одну", 1.0),
    ("одного", 1.0),
    ("два", 2.0),
    ("две", 2.0),
    ("двух", 2.0),
    ("три", 3.0),
    ("трех", 3.0),
    ("четыре", 4.0),
    ("четырех", 4.0),
    ("пять", 5.0),
    ("пяти", 5.0),
    ("шесть", 6.0),
    ("шести", 6.0),
    ("семь", 7.0),
    ("семи", 7.0),
    ("восемь", 8.0),
    ("восьми", 8.0),
    ("девять", 9.0),
    ("девяти", 9.0),
    ("десять", 10.0),
    ("десяти", 10.0),
    ("одиннадцать", 11.0),
    ("двенадцать", 12.0),
    ("тринадцать", 13.0),
    ("четырнадцать", 14.0),
    ("пятнадцать", 15.0),
    ("шестнадцать", 16.0),
    ("семнадцать", 17.0),
    ("восемнадцать", 18.0),
    ("девятнадцать", 19.0),
    ("двадцать", 20.0),
    ("тридцать", 30.0),
    ("сорок", 40.0),
    ("пятьдесят", 50.0),
    ("шестьдесят", 60.0),
    ("семьдесят", 70.0),
    ("восемьдесят", 80.0),
    ("девяносто", 90.0),
    ("сто", 100.0),
];

fn number_word(word: &str) -> Option<f64> {
    NUMBERS.iter().find(|(name, _)| *name == word).map(|(_, value)| *value)
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

// `\d` is ASCII in the TS patterns (no `u` flag): [0-9] here, as Rust's `\d` is any Unicode digit.
static CLOCK_DOT: LazyLock<Regex> = LazyLock::new(|| regex(r"([0-9])[.:]([0-9])"));
static NOT_WORD: LazyLock<Regex> = LazyLock::new(|| regex(r"[^\p{L}\p{N}:+\s-]"));
static LOOSE_HYPHEN: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+-\s+|\s+-|-\s+"));
static WAKE_PREFIX: LazyLock<Regex> = LazyLock::new(|| regex(r"^(алиса|яндекс)\s+"));
/// Taken out as whole words; word edges are spaces (the TS pattern looked ahead for them: no look-around here).
const POLITE: &[&str] = &["пожалуйста", "плиз"];

/// Lower case, ё as е, no punctuation but the colon of a clock time, single spaces, no wake word or "please".
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase().replace('ё', "е");
    let clock = CLOCK_DOT.replace_all(&lower, "${1}:${2}");
    let cleaned = NOT_WORD.replace_all(&clock, " ");
    let joined = LOOSE_HYPHEN.replace_all(&cleaned, " ");
    // The polite words go with the spaces around them, then single spaces and no edges: one pass over the words.
    let words: Vec<&str> =
        joined.split(char::is_whitespace).filter(|word| !word.is_empty() && !POLITE.contains(word)).collect();
    WAKE_PREFIX.replace(&words.join(" "), "").into_owned()
}

/// A number read from words: its value and the index of the word after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberRead {
    pub value: f64,
    pub next: usize,
}

/// A number in digits or words at the start of `words`: "25", "двадцать пять".
pub fn read_number(words: &[&str], start: usize) -> Option<NumberRead> {
    let word = *words.get(start)?;
    if !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()) {
        return Some(NumberRead { value: word.parse().ok()?, next: start + 1 });
    }
    let value = number_word(word)?;
    // "двадцать пять": tens, then units.
    let unit = words.get(start + 1).and_then(|next| number_word(next));
    if let Some(unit) = unit {
        if value >= 20.0 && value % 10.0 == 0.0 && value < 100.0 && unit > 0.0 && unit < 10.0 {
            return Some(NumberRead { value: value + unit, next: start + 2 });
        }
    }
    Some(NumberRead { value, next: start + 1 })
}

/// The first number anywhere in the text.
pub fn find_number(text: &str) -> Option<f64> {
    let words: Vec<&str> = text.split(' ').collect();
    (0..words.len()).find_map(|i| read_number(&words, i)).map(|found| found.value)
}

fn unit_seconds(word: Option<&str>) -> Option<f64> {
    match word? {
        "час" | "часа" | "часов" | "ч" => Some(3600.0),
        "минута" | "минуту" | "минуты" | "минут" | "мин" => Some(60.0),
        "секунда" | "секунду" | "секунды" | "секунд" | "сек" => Some(1.0),
        _ => None,
    }
}

/// A duration in seconds: "5 минут", "пять минут", "минуту", "полчаса", "полторы минуты", "1 час 30 минут",
/// "час двадцать". None when there is none.
pub fn parse_duration(text: &str) -> Option<u64> {
    let normalized = normalize(text);
    let words: Vec<&str> = normalized.split(' ').collect();
    let mut total = 0.0;
    let mut found = false;
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        if word == "полчаса" {
            total += 1800.0;
            found = true;
            i += 1;
            continue;
        }
        if word == "полтора" || word == "полторы" {
            if let Some(unit) = unit_seconds(words.get(i + 1).copied()) {
                total += unit * 1.5;
                found = true;
                i += 1;
            }
            i += 1;
            continue;
        }
        if let Some(number) = read_number(&words, i) {
            if let Some(unit) = unit_seconds(words.get(number.next).copied()) {
                total += number.value * unit;
                found = true;
                i = number.next + 1;
                continue;
            }
            // "час двадцать": minutes after an hour without a unit of their own.
            if found && total % 3600.0 == 0.0 && number.value < 60.0 && number.next == words.len() {
                total += number.value * 60.0;
                i = number.next;
            }
            i += 1;
            continue;
        }
        // A unit alone: "минуту", "час".
        if let Some(unit) = unit_seconds(Some(word)) {
            if !(i > 0 && read_number(&words, i - 1).is_some()) {
                total += unit;
                found = true;
            }
        }
        i += 1;
    }
    found.then(|| js::round(total).max(0.0) as u64)
}

/// A duration as said: «1 минута 5 секунд»; after «через» — «1 минуту 5 секунд» (`after`).
pub fn say_duration(seconds: i64, after: bool) -> String {
    let mut parts = Vec::new();
    let hours = seconds.div_euclid(3600);
    let minutes = (seconds % 3600).div_euclid(60);
    let rest = seconds % 60;
    if hours != 0 {
        parts.push(format!("{hours} {}", plural(hours, "час", "часа", "часов")));
    }
    if minutes != 0 {
        parts
            .push(format!("{minutes} {}", plural(minutes, if after { "минуту" } else { "минута" }, "минуты", "минут")));
    }
    if rest != 0 || parts.is_empty() {
        parts.push(format!("{rest} {}", plural(rest, if after { "секунду" } else { "секунда" }, "секунды", "секунд")));
    }
    parts.join(" ")
}

/// The Russian plural of `n`: one, few (2–4) or many.
pub fn plural<'a>(n: i64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let tens = n % 100;
    let units = n % 10;
    if (11..=14).contains(&tens) {
        return many;
    }
    if units == 1 {
        return one;
    }
    if (2..=4).contains(&units) {
        return few;
    }
    many
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTime {
    pub hours: u32,
    pub minutes: u32,
}

static CLOCK_COLON: LazyLock<Regex> = LazyLock::new(|| regex(r"([0-9]{1,2}):([0-9]{2})"));

/// A clock time: "7:30", "в 7 30", "семь тридцать", "в 9 вечера"; "в половине восьмого" is not supported.
/// Evening and afternoon words move hours after noon.
pub fn parse_clock(text: &str) -> Option<ClockTime> {
    let normalized = normalize(text);
    let mut hours: Option<f64> = None;
    let mut minutes = 0.0;
    if let Some(colon) = CLOCK_COLON.captures(&normalized) {
        hours = colon[1].parse().ok();
        minutes = colon[2].parse().unwrap_or(0.0);
    } else {
        let words: Vec<&str> = normalized.split(' ').collect();
        for i in 0..words.len() {
            let Some(first) = read_number(&words, i) else { continue };
            if first.value > 23.0 {
                continue;
            }
            let after = words.get(first.next).copied().unwrap_or("");
            // "через 5 минут" is a duration, not a time.
            if matches!(after, "минут" | "минуты" | "минуту" | "секунд" | "секунды" | "секунду")
            {
                continue;
            }
            hours = Some(first.value);
            let mut next = first.next;
            if matches!(after, "час" | "часа" | "часов") {
                next += 1;
            }
            if let Some(second) = read_number(&words, next) {
                if second.value < 60.0 {
                    minutes = second.value;
                }
            }
            break;
        }
    }
    let mut hours = hours?;
    if hours > 23.0 || minutes > 59.0 {
        return None;
    }
    if (normalized.contains("вечера") || normalized.contains("дня") || normalized.contains("после обеда"))
        && hours < 12.0
    {
        hours += 12.0;
    }
    if normalized.contains("ночи") && hours == 12.0 {
        hours = 0.0;
    }
    Some(ClockTime { hours: hours as u32, minutes: minutes as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_a_command_case_yo_punctuation_wake_word_please() {
        assert_eq!(normalize("Алиса, сделай громче, пожалуйста!"), "сделай громче");
        assert_eq!(normalize("Ещё 7:30."), "еще 7:30");
        assert_eq!(normalize("Пожалуйста, пожалуйста стоп"), "стоп");
        assert_eq!(normalize("пожалуйста-то"), "пожалуйста-то");
        assert_eq!(normalize("Яндекс,  плиз —  громче"), "громче");
        assert_eq!(normalize("рок - н - ролл"), "рок н ролл");
        assert_eq!(normalize("1.2.3"), "1:2 3");
    }

    #[test]
    fn reads_numbers_in_digits_and_words() {
        assert_eq!(find_number("громкость 7"), Some(7.0));
        assert_eq!(find_number("на двадцать пять"), Some(25.0));
        assert_eq!(find_number("на два"), Some(2.0));
        assert_eq!(find_number("без чисел"), None);
    }

    #[test]
    fn reads_durations() {
        assert_eq!(parse_duration("5 минут"), Some(300));
        assert_eq!(parse_duration("пять минут"), Some(300));
        assert_eq!(parse_duration("минуту"), Some(60));
        assert_eq!(parse_duration("полчаса"), Some(1800));
        assert_eq!(parse_duration("полторы минуты"), Some(90));
        assert_eq!(parse_duration("1 час 30 минут"), Some(5400));
        assert_eq!(parse_duration("час двадцать"), Some(4800));
        assert_eq!(parse_duration("тридцать секунд"), Some(30));
        assert_eq!(parse_duration("завтра"), None);
    }

    #[test]
    fn reads_clock_times() {
        assert_eq!(parse_clock("7:30"), Some(ClockTime { hours: 7, minutes: 30 }));
        assert_eq!(parse_clock("семь тридцать"), Some(ClockTime { hours: 7, minutes: 30 }));
        assert_eq!(parse_clock("9 вечера"), Some(ClockTime { hours: 21, minutes: 0 }));
        assert_eq!(parse_clock("7 часов 15 минут"), Some(ClockTime { hours: 7, minutes: 15 }));
        assert_eq!(parse_clock("через 5 минут"), None);
    }

    #[test]
    fn says_durations_back() {
        assert_eq!(say_duration(60, false), "1 минута");
        assert_eq!(say_duration(61, true), "1 минуту 1 секунду");
        assert_eq!(say_duration(300, false), "5 минут");
        assert_eq!(say_duration(5400, false), "1 час 30 минут");
        assert_eq!(say_duration(42, false), "42 секунды");
    }
}
