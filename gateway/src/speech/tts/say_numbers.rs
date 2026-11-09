//! Numbers in words for our own engines: Silero skips digits («осталось 53 секунды» lost its number, live 02.11), and
//! our skills and Yandex's texts write them as digits. Russian agreement is kept where a phrase shows it: the gender
//! of 1 and 2 by the noun after («одна минута», «две секунды», «одну минуту»), the genitive after «от», «до» and the
//! like («от одного до шести градусов»). Ordinals (dates, years) stay cardinal.

use std::sync::LazyLock;

use regex::Regex;

use crate::skills::text::plural;
use crate::speech::asr::transcript::is_js_space;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Masculine,
    Feminine,
    Neuter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Nominative,
    Genitive,
}

const UNITS_NOM: [&str; 10] = ["ноль", "один", "два", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять"];
const UNITS_GEN: [&str; 10] =
    ["ноля", "одного", "двух", "трёх", "четырёх", "пяти", "шести", "семи", "восьми", "девяти"];
const TEENS: [&str; 10] = [
    "десять",
    "одиннадцать",
    "двенадцать",
    "тринадцать",
    "четырнадцать",
    "пятнадцать",
    "шестнадцать",
    "семнадцать",
    "восемнадцать",
    "девятнадцать",
];
const TENS_NOM: [&str; 10] =
    ["", "", "двадцать", "тридцать", "сорок", "пятьдесят", "шестьдесят", "семьдесят", "восемьдесят", "девяносто"];
const TENS_GEN: [&str; 10] =
    ["", "", "двадцати", "тридцати", "сорока", "пятидесяти", "шестидесяти", "семидесяти", "восьмидесяти", "девяноста"];
const HUNDREDS_NOM: [&str; 10] =
    ["", "сто", "двести", "триста", "четыреста", "пятьсот", "шестьсот", "семьсот", "восемьсот", "девятьсот"];
const HUNDREDS_GEN: [&str; 10] =
    ["", "ста", "двухсот", "трёхсот", "четырёхсот", "пятисот", "шестисот", "семисот", "восьмисот", "девятисот"];

struct Scale {
    value: u64,
    gender: Gender,
    nominative: [&'static str; 3],
    genitive: [&'static str; 3],
}

const SCALES: [Scale; 3] = [
    Scale {
        value: 1_000_000_000,
        gender: Gender::Masculine,
        nominative: ["миллиард", "миллиарда", "миллиардов"],
        genitive: ["миллиарда", "миллиардов", "миллиардов"],
    },
    Scale {
        value: 1_000_000,
        gender: Gender::Masculine,
        nominative: ["миллион", "миллиона", "миллионов"],
        genitive: ["миллиона", "миллионов", "миллионов"],
    },
    Scale {
        value: 1_000,
        gender: Gender::Feminine,
        nominative: ["тысяча", "тысячи", "тысяч"],
        genitive: ["тысячи", "тысяч", "тысяч"],
    },
];

type Words = [&'static str; 10];

fn by_case(case: Case, nominative: &'static Words, genitive: &'static Words) -> &'static Words {
    if case == Case::Genitive {
        genitive
    } else {
        nominative
    }
}

fn unit(digit: usize, gender: Gender, case: Case) -> &'static str {
    match (digit, gender, case) {
        (1, Gender::Feminine, Case::Genitive) => "одной",
        (1, _, Case::Genitive) => "одного",
        (1, Gender::Feminine, _) => "одна",
        (1, Gender::Neuter, _) => "одно",
        (2, Gender::Feminine, Case::Nominative) => "две",
        _ => by_case(case, &UNITS_NOM, &UNITS_GEN)[digit],
    }
}

/// 1–999 in words. A larger count (a thousand billions and up) has no word for its hundreds: that place is left
/// empty, as the TS left it.
fn below_1000(n: u64, gender: Gender, case: Case, words: &mut Vec<String>) {
    let hundreds = (n / 100) as usize;
    let rest = (n % 100) as usize;
    if hundreds > 0 {
        words.push(by_case(case, &HUNDREDS_NOM, &HUNDREDS_GEN).get(hundreds).copied().unwrap_or("").to_string());
    }
    if (10..20).contains(&rest) {
        let teen = TEENS[rest - 10];
        words.push(if case == Case::Genitive {
            format!("{}и", teen.trim_end_matches('ь'))
        } else {
            teen.to_string()
        });
    } else {
        if rest >= 20 {
            words.push(by_case(case, &TENS_NOM, &TENS_GEN)[rest / 10].to_string());
        }
        if !rest.is_multiple_of(10) {
            words.push(unit(rest % 10, gender, case).to_string());
        }
    }
}

/// A number as JavaScript's String(n) writes it: digits, or an exponent from 1e21 on.
fn js_number(n: f64) -> String {
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n.abs() >= 1e21 {
        let text = format!("{n:e}");
        return match text.split_once('e') {
            Some((digits, exponent)) if !exponent.starts_with('-') => format!("{digits}e+{exponent}"),
            _ => text,
        };
    }
    format!("{n}")
}

/// A whole number in words: «двадцать пять», «одна» (feminine), «двух» (genitive).
pub fn cardinal(n: f64, gender: Gender, case: Case) -> String {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if n.fract() != 0.0 || !(0.0..=MAX_SAFE).contains(&n) {
        return js_number(n);
    }
    let n = n as u64;
    if n == 0 {
        return by_case(case, &UNITS_NOM, &UNITS_GEN)[0].to_string();
    }
    let mut words = Vec::new();
    let mut rest = n;
    for scale in &SCALES {
        let count = rest / scale.value;
        if count == 0 {
            continue;
        }
        rest %= scale.value;
        below_1000(count, scale.gender, case, &mut words);
        let [one, few, many] = if case == Case::Genitive { scale.genitive } else { scale.nominative };
        words.push(plural(count as i64, one, few, many).to_string());
    }
    if rest > 0 {
        below_1000(rest, gender, case, &mut words);
    }
    words.join(" ")
}

/// After these a number is in the genitive: «от одного до шести», «около пяти».
const GENITIVE_BEFORE: &[&str] = &[
    "от",
    "до",
    "из",
    "около",
    "после",
    "без",
    "для",
    "у",
    "с",
    "со",
    "более",
    "менее",
    "больше",
    "меньше",
    "свыше",
    "кроме",
    "против",
];

/// Short words after a number that are not its noun: «от 1 до 6 градусов».
const NOT_A_NOUN: &[&str] =
    &["до", "от", "и", "или", "по", "на", "в", "во", "с", "со", "к", "из", "за", "а", "но", "же"];

/// The gender the noun after a number asks of 1 and 2, by its ending: «минута», «минуту», «минуты» are feminine.
fn gender_of(n: f64, next: &str) -> Gender {
    let units = n % 10.0;
    let tens = n % 100.0;
    let last = next.chars().last();
    if (11.0..=14.0).contains(&tens) || NOT_A_NOUN.contains(&next) {
        Gender::Masculine
    } else if units == 1.0 && matches!(last, Some('а' | 'я' | 'у' | 'ю')) {
        Gender::Feminine
    } else if units == 1.0 && matches!(last, Some('о' | 'е' | 'ё')) {
        Gender::Neuter
    } else if units == 2.0 && matches!(last, Some('ы' | 'и')) {
        Gender::Feminine
    } else {
        Gender::Masculine
    }
}

/// Number("…") of a run of ASCII digits.
fn number(digits: &str) -> f64 {
    digits.parse().unwrap_or(f64::NAN)
}

/// A whole number among its words: its gender from the noun after, its case from the word before.
fn in_context(digits: &str, before: &str, next: &str) -> String {
    let n = number(digits);
    let case =
        if GENITIVE_BEFORE.contains(&before.to_lowercase().as_str()) { Case::Genitive } else { Case::Nominative };
    let next = next.to_lowercase();
    let mut words = cardinal(n, gender_of(n, &next), case);
    // «через 1 минуту»: the accusative of a feminine one.
    if n % 10.0 == 1.0 && n % 100.0 != 11.0 && case == Case::Nominative && next.ends_with(['у', 'ю']) {
        if let Some(stem) = words.strip_suffix("одна") {
            words = format!("{stem}одну");
        }
    }
    words
}

/// Units after a number, said in words agreeing with it.
fn unit_forms(sign: &str) -> Option<[&'static str; 3]> {
    if sign == "%" {
        Some(["процент", "процента", "процентов"])
    } else if sign.starts_with('°') {
        Some(["градус", "градуса", "градусов"])
    } else if sign == "м/с" {
        Some(["метр в секунду", "метра в секунду", "метров в секунду"])
    } else if sign == "км/ч" {
        Some(["километр в час", "километра в час", "километров в час"])
    } else {
        None
    }
}

static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").expect("a valid pattern"));

fn is_letter(c: char) -> bool {
    LETTER.is_match(c.encode_utf8(&mut [0; 4]))
}

fn digit(c: Option<&char>) -> bool {
    c.is_some_and(char::is_ascii_digit)
}

/// A word character of JavaScript's `\b`: ASCII letters, digits and the underscore.
fn word_char(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
}

fn text(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Clock times: «7:05» → «семь ноль пять», «21:00» → «двадцать один ноль ноль» (`\b(\d{1,2}):(\d{2})\b`).
fn clock_times(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || !word_char(chars.get(i - 1));
        let found = [2, 1].into_iter().find(|&hours| {
            boundary
                && (0..hours).all(|k| digit(chars.get(i + k)))
                && chars.get(i + hours) == Some(&':')
                && digit(chars.get(i + hours + 1))
                && digit(chars.get(i + hours + 2))
                && !word_char(chars.get(i + hours + 3))
        });
        let Some(hours) = found else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let m = number(&text(&chars[i + hours + 1..i + hours + 3]));
        let minutes = if m == 0.0 {
            "ноль ноль".to_string()
        } else if m < 10.0 {
            format!("ноль {}", cardinal(m, Gender::Masculine, Case::Nominative))
        } else {
            cardinal(m, Gender::Masculine, Case::Nominative)
        };
        let hours_said = cardinal(number(&text(&chars[i..i + hours])), Gender::Masculine, Case::Nominative);
        out.extend(format!("{hours_said} {minutes}").chars());
        i += hours + 3;
    }
    out
}

/// Signs: «+6» → «плюс шесть», «−3» → «минус три»; a hyphen between numbers is a range, not a minus
/// (`(^|[\s(«"])([+−-])(?=\d)`).
fn signs(chars: &[char]) -> Vec<char> {
    let sign_word = |c: char| if c == '+' { "плюс" } else { "минус" };
    let is_sign = |c: Option<&char>| matches!(c, Some('+' | '−' | '-'));
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if i == 0 && is_sign(chars.first()) && digit(chars.get(1)) {
            out.extend(format!("{} ", sign_word(chars[0])).chars());
            i = 1;
            continue;
        }
        let opens = is_js_space(chars[i]) || matches!(chars[i], '(' | '«' | '"');
        if opens && is_sign(chars.get(i + 1)) && digit(chars.get(i + 2)) {
            out.push(chars[i]);
            out.extend(format!("{} ", sign_word(chars[i + 1])).chars());
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Fractions: «4,5» → «четыре и пять» (`(\d+)[.,](\d+)`).
fn fractions(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if !digit(chars.get(i)) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut whole_end = i;
        while digit(chars.get(whole_end)) {
            whole_end += 1;
        }
        if !matches!(chars.get(whole_end), Some('.' | ',')) || !digit(chars.get(whole_end + 1)) {
            // Every start inside this run fails the same way.
            out.extend_from_slice(&chars[i..whole_end]);
            i = whole_end;
            continue;
        }
        let mut part_end = whole_end + 1;
        while digit(chars.get(part_end)) {
            part_end += 1;
        }
        let whole = cardinal(number(&text(&chars[i..whole_end])), Gender::Masculine, Case::Nominative);
        let part = cardinal(number(&text(&chars[whole_end + 1..part_end])), Gender::Masculine, Case::Nominative);
        out.extend(format!("{whole} и {part}").chars());
        i = part_end;
    }
    out
}

/// The unit after a number at `at`: `\s?(%|°\s?[CС]?(?!\p{L})|м\/с(?!\p{L})|км\/ч(?!\p{L}))` — the sign (without
/// the space before it) and where it ends.
fn unit_at(chars: &[char], at: usize) -> Option<(String, usize)> {
    let not_letter = |index: usize| !chars.get(index).copied().is_some_and(is_letter);
    let starts = |index: usize, word: &str| word.chars().enumerate().all(|(k, c)| chars.get(index + k) == Some(&c));
    let space = |index: usize| chars.get(index).copied().is_some_and(is_js_space);
    let mut starts_at = vec![at];
    if space(at) {
        starts_at.insert(0, at + 1);
    }
    for start in starts_at {
        if chars.get(start) == Some(&'%') {
            return Some(("%".into(), start + 1));
        }
        if chars.get(start) == Some(&'°') {
            let mut after_space = vec![start + 1];
            if space(start + 1) {
                after_space.insert(0, start + 2);
            }
            for c_at in after_space {
                let mut ends = vec![c_at];
                if matches!(chars.get(c_at), Some('C' | 'С')) {
                    ends.insert(0, c_at + 1);
                }
                if let Some(end) = ends.into_iter().find(|&end| not_letter(end)) {
                    return Some((text(&chars[start..end]), end));
                }
            }
        }
        for sign in ["м/с", "км/ч"] {
            let end = start + sign.chars().count();
            if starts(start, sign) && not_letter(end) {
                return Some((sign.to_string(), end));
            }
        }
    }
    None
}

/// The word of letters right before `at`, with nothing but marks and spaces between (`(\p{L}+)[^\p{L}\d]*$`).
fn word_before(chars: &[char], at: usize) -> String {
    let mut end = at;
    while end > 0 && !is_letter(chars[end - 1]) && !chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_letter(chars[start - 1]) {
        start -= 1;
    }
    text(&chars[start..end])
}

/// The word of letters right after `at`, with nothing but marks and spaces between (`^[^\p{L}\d]*(\p{L}+)`).
fn word_after(chars: &[char], at: usize) -> String {
    let mut start = at;
    while start < chars.len() && !is_letter(chars[start]) && !chars[start].is_ascii_digit() {
        start += 1;
    }
    let mut end = start;
    while end < chars.len() && is_letter(chars[end]) {
        end += 1;
    }
    text(&chars[start..end])
}

/// Whole numbers, with a unit after them («25%», «−3 °C», «4 м/с») or among words.
fn whole_numbers(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if !digit(chars.get(i)) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut end = i;
        while digit(chars.get(end)) {
            end += 1;
        }
        let digits = text(&chars[i..end]);
        let before = word_before(chars, i);
        let (said, next_at) =
            match unit_at(chars, end).and_then(|(sign, unit_end)| Some((unit_forms(&sign)?, unit_end))) {
                Some(([one, few, many], unit_end)) => {
                    // The unit's form after the number: skills/text's plural, as the TS imported it. Whole digits
                    // make an integer.
                    let form = plural(number(&digits) as i64, one, few, many);
                    (format!("{} {form}", in_context(&digits, &before, "")), unit_end)
                }
                None => (in_context(&digits, &before, &word_after(chars, end)), end),
            };
        out.extend(said.chars());
        i = next_at;
    }
    out
}

/// The text with its numbers in words, for an engine that does not say digits.
pub fn say_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let said: String = whole_numbers(&fractions(&signs(&clock_times(&chars)))).into_iter().collect();
    said.split(is_js_space).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_each_text_with_its_numbers_in_words() {
        let cases = [
            // Our skills' answers (live, 02.11: Silero said «Осталось секунды»).
            ("Осталось 53 секунды.", "Осталось пятьдесят три секунды."),
            ("Хорошо. 1 минута. Отсчёт пошёл.", "Хорошо. одна минута. Отсчёт пошёл."),
            ("Хорошо, напомню через 1 минуту.", "Хорошо, напомню через одну минуту."),
            ("Осталось 22 секунды.", "Осталось двадцать две секунды."),
            ("Таймер на 2 часа 21 минуту.", "Таймер на два часа двадцать одну минуту."),
            ("Будильник на 7:05 завтра.", "Будильник на семь ноль пять завтра."),
            ("Будильник на 21:00.", "Будильник на двадцать один ноль ноль."),
            ("Громкость 11.", "Громкость одиннадцать."),
            // Yandex's texts.
            ("Сейчас в Казани +6, ветер 4 м/с.", "Сейчас в Казани плюс шесть, ветер четыре метра в секунду."),
            ("Днём от 1 до 6 градусов.", "Днём от одного до шести градусов."),
            ("Ночью -3 °C.", "Ночью минус три градуса."),
            ("Заряд 100%, влажность 21%.", "Заряд сто процентов, влажность двадцать один процент."),
            ("Курс 92,5.", "Курс девяносто два и пять."),
            ("Песня 1-2", "Песня один-два"),
            ("Без цифр.", "Без цифр."),
        ];
        for (text, said) in cases {
            assert_eq!(say_numbers(text), said, "{text}");
        }
    }

    #[test]
    fn says_whole_numbers_of_any_size_by_gender_and_case() {
        use Case::*;
        use Gender::*;
        assert_eq!(cardinal(0.0, Masculine, Nominative), "ноль");
        assert_eq!(cardinal(2026.0, Masculine, Nominative), "две тысячи двадцать шесть");
        assert_eq!(cardinal(1_000_001.0, Masculine, Nominative), "один миллион один");
        assert_eq!(cardinal(2.0, Feminine, Nominative), "две");
        assert_eq!(cardinal(1.0, Neuter, Nominative), "одно");
        assert_eq!(cardinal(111.0, Masculine, Genitive), "ста одиннадцати");
        assert_eq!(cardinal(45.0, Masculine, Genitive), "сорока пяти");
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(cardinal(12_345_678_901_234_567_890.0, Masculine, Nominative), "12345678901234567000");
    }
}
