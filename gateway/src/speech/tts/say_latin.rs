//! Latin words in Russian letters for our own engines: Silero's Russian model drops Latin («Это Carpenter Brut» came
//! out as «Это», live 02.11), and track names, artists and brands come in Latin. Read by English rules, roughly:
//! «Carpenter Brut» → «Карпентер Брут», «L.A» → «эл эй», «80's» → «восьмидесятых». Not a dictionary of English: a
//! name comes out recognisable, not always as an Englishman says it.

use std::sync::LazyLock;

use regex::Regex;

/// Words the rules get wrong, frequent in titles, or brands said in Russian speech.
fn known(word: &str) -> Option<&'static str> {
    Some(match word {
        "the" => "зэ",
        "a" => "э",
        "i" => "ай",
        "you" => "ю",
        "your" => "ёр",
        "of" => "оф",
        "and" => "энд",
        "to" => "ту",
        "is" => "из",
        "my" => "май",
        "me" => "ми",
        "we" => "уи",
        "he" => "хи",
        "she" => "ши",
        "be" => "би",
        "are" => "ар",
        "what" => "уот",
        "who" => "ху",
        "do" => "ду",
        "dont" => "донт",
        "love" => "лав",
        "one" => "уан",
        "two" => "ту",
        "world" => "уорлд",
        "life" => "лайф",
        "heart" => "харт",
        "people" => "пипл",
        "baby" => "бейби",
        "come" => "кам",
        "some" => "сам",
        "money" => "мани",
        "bluetooth" => "блютуз",
        "youtube" => "ютуб",
        "google" => "гугл",
        "iphone" => "айфон",
        "wifi" => "вайфай",
        "ok" => "окей",
        "okay" => "окей",
        "dj" => "диджей",
        "feat" => "фит",
        _ => return None,
    })
}

/// Letters by name, for abbreviations: «L.A», «BBC».
const LETTER_NAMES: [&str; 26] = [
    "эй",
    "би",
    "си",
    "ди",
    "и",
    "эф",
    "джи",
    "эйч",
    "ай",
    "джей",
    "кей",
    "эл",
    "эм",
    "эн",
    "оу",
    "пи",
    "кью",
    "ар",
    "эс",
    "ти",
    "ю",
    "ви",
    "дабл ю",
    "экс",
    "уай",
    "зед",
];

const DECADES: [&str; 10] = [
    "",
    "десятых",
    "двадцатых",
    "тридцатых",
    "сороковых",
    "пятидесятых",
    "шестидесятых",
    "семидесятых",
    "восьмидесятых",
    "девяностых",
];

/// Letter groups, longest first.
const GROUPS: &[(&str, &str)] = &[
    ("tion", "шн"),
    ("sch", "ш"),
    ("tch", "ч"),
    ("igh", "ай"),
    ("dge", "дж"),
    ("sh", "ш"),
    ("ch", "ч"),
    ("th", "т"),
    ("ph", "ф"),
    ("ck", "к"),
    ("qu", "кв"),
    ("wh", "у"),
    ("wr", "р"),
    ("gh", ""),
    ("oo", "у"),
    ("ee", "и"),
    ("ea", "и"),
    ("ai", "эй"),
    ("ay", "эй"),
    ("ey", "эй"),
    ("ou", "ау"),
    ("ow", "оу"),
    ("oy", "ой"),
    ("oi", "ой"),
    ("au", "о"),
    ("aw", "о"),
    ("ew", "ю"),
];

/// Each letter's own sound.
const LETTERS: [&str; 26] = [
    "а", "б", "к", "д", "е", "ф", "г", "х", "и", "дж", "к", "л", "м", "н", "о", "п", "к", "р", "с", "т", "у", "в", "у",
    "кс", "и", "з",
];

/// The long vowel of a one-syllable word with a silent «e»: «time» → «тайм», «late» → «лейт».
fn magic_e(vowel: u8) -> Option<&'static str> {
    match vowel {
        b'a' => Some("эй"),
        b'e' => Some("и"),
        b'i' => Some("ай"),
        b'o' => Some("оу"),
        b'u' => Some("ю"),
        _ => None,
    }
}

fn is_vowel(letter: Option<u8>) -> bool {
    matches!(letter, Some(b'a' | b'e' | b'i' | b'o' | b'u' | b'y'))
}

fn sound(letter: u8) -> &'static str {
    if letter.is_ascii_lowercase() {
        LETTERS[usize::from(letter - b'a')]
    } else {
        ""
    }
}

/// Runs of vowels: where they start and how long they are.
fn vowel_runs(letters: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut at = 0;
    while at < letters.len() {
        if is_vowel(Some(letters[at])) {
            let start = at;
            while at < letters.len() && is_vowel(Some(letters[at])) {
                at += 1;
            }
            runs.push((start, at - start));
        } else {
            at += 1;
        }
    }
    runs
}

/// One lowercase Latin word in Russian letters.
fn word(latin: &str) -> String {
    if let Some(known) = known(latin) {
        return known.to_string();
    }
    let latin = latin.as_bytes();
    let mut letters = latin;
    let mut magic = None;
    // A final «e» after a consonant is silent («venice», «little»); in a one-syllable word it makes its vowel long.
    let silent_e = letters.len() > 2 && letters.ends_with(b"e") && !is_vowel(Some(letters[letters.len() - 2]));
    if silent_e {
        letters = &letters[..letters.len() - 1];
        let runs = vowel_runs(letters);
        if let [(start, 1)] = runs[..] {
            if magic_e(letters[start]).is_some() {
                magic = Some(start);
            }
        }
    }
    let one_syllable = vowel_runs(letters).len() == 1;
    let at_index = |index: usize| letters.get(index).copied();
    let mut out = String::new();
    let mut at = 0;
    while at < letters.len() {
        let rest = &letters[at..];
        let group =
            if magic == Some(at) { None } else { GROUPS.iter().find(|(from, _)| rest.starts_with(from.as_bytes())) };
        // «ir», «ur» before a consonant in one syllable: «girl» → «гёрл», «burn» → «бёрн»; «Nirvana» keeps its «и».
        let r_colored =
            one_syllable && at > 0 && matches!(rest, [b'i' | b'u', b'r', ..]) && !is_vowel(rest.get(2).copied());
        if magic == Some(at) {
            out.push_str(magic_e(letters[at]).unwrap_or(""));
            at += 1;
        } else if r_colored {
            out.push_str("ёр");
            at += 2;
        } else if let Some((from, said)) = group {
            out.push_str(said);
            at += from.len();
        } else {
            let letter = letters[at];
            let before = if at > 0 { at_index(at - 1) } else { None };
            let next = latin.get(at + 1).copied();
            let after = &letters[at + 1..];
            let mut said = sound(letter);
            if Some(letter) == before && !is_vowel(Some(letter)) {
                said = ""; // «kiss», «hello»: one sound
            } else if letter == b'c' && matches!(next, Some(b'e' | b'i' | b'y')) {
                said = "с";
            } else if letter == b'g' && silent_e && at == letters.len() - 1 {
                said = "дж"; // «page», «stage»
            } else if letter == b's' && is_vowel(before) && is_vowel(next) {
                said = "з"; // «music», «rose»
            } else if letter == b'e' && (at == 0 || is_vowel(before)) {
                said = "э";
            } else if letter == b'y' && at == 0 && is_vowel(next) {
                said = "й";
            } else if letter == b'y'
                && at == letters.len() - 1
                && !letters.iter().any(|l| matches!(l, b'a' | b'e' | b'i' | b'o' | b'u'))
            {
                said = "ай"; // «sky»
            } else if letter == b'u'
                && ["n", "m", "ck", "sh", "ff", "tt", "gg", "bb", "dd", "zz", "ss", "st"]
                    .iter()
                    .any(|start| after.starts_with(start.as_bytes()))
            {
                said = "а"; // «punk»
            } else if letter == b'a' && (after == b"ll" || after == b"lls") {
                said = "о"; // «all», «ball», not «Metallica»
            } else if letter == b'h' && at == letters.len() - 1 && is_vowel(before) {
                said = ""; // «oh»
            } else if letter == b'e'
                && after == b"s"
                && before == Some(b'l')
                && !is_vowel(if at >= 2 { at_index(at - 2) } else { None })
            {
                said = ""; // «beatles»
            }
            out.push_str(said);
            at += 1;
        }
    }
    out
}

fn spell(letters: &str) -> String {
    letters
        .chars()
        .filter_map(|letter| {
            let letter = letter.to_ascii_lowercase();
            letter.is_ascii_lowercase().then(|| LETTER_NAMES[usize::from(letter as u8 - b'a')])
        })
        .collect::<Vec<_>>()
        .join(" ")
}

static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").expect("a valid pattern"));

/// A letter or an ASCII digit, as `/[\p{L}\d]/u` tested one UTF-16 unit (a character past the BMP never matched).
fn letter_or_digit(c: Option<char>) -> bool {
    c.is_some_and(|c| u32::from(c) <= 0xffff && (c.is_ascii_digit() || LETTER.is_match(c.encode_utf8(&mut [0; 4]))))
}

fn latin_letter(c: Option<&char>) -> bool {
    c.is_some_and(char::is_ascii_alphabetic)
}

/// «AC/DC»: the slash between Latin letters is not said.
fn slashes(chars: &[char]) -> Vec<char> {
    (0..chars.len())
        .map(|i| {
            let between = i > 0 && latin_letter(chars.get(i - 1)) && latin_letter(chars.get(i + 1));
            if chars[i] == '/' && between {
                ' '
            } else {
                chars[i]
            }
        })
        .collect()
}

/// A decade at `i`: `(?<![\d.,])(?:1\d|20)?([1-9])0['’]?s(?![A-Za-z])` — its end and its tens.
fn decade_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if i > 0 && matches!(chars[i - 1], '0'..='9' | '.' | ',') {
        return None;
    }
    let at = |index: usize| chars.get(index).copied();
    let rest = |start: usize| -> Option<(usize, usize)> {
        let tens = at(start).filter(|c| ('1'..='9').contains(c))?;
        if at(start + 1) != Some('0') {
            return None;
        }
        let apostrophe = matches!(at(start + 2), Some('\'' | '’'));
        for s in if apostrophe { vec![start + 3, start + 2] } else { vec![start + 2] } {
            if at(s) == Some('s') && !latin_letter(chars.get(s + 1)) {
                return Some((s + 1, tens as usize - '0' as usize));
            }
        }
        None
    };
    let century = match (at(i), at(i + 1)) {
        (Some('1'), Some(d)) if d.is_ascii_digit() => rest(i + 2),
        (Some('2'), Some('0')) => rest(i + 2),
        _ => None,
    };
    century.or_else(|| rest(i))
}

fn decades(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match decade_at(chars, i) {
            Some((end, tens)) => {
                out.extend(DECADES[tens].chars());
                i = end;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// Letters with dots at `i`: `(?<![A-Za-z])[A-Za-z](?:\.[A-Za-z])+(?![A-Za-z])` — where they end.
fn dotted_at(chars: &[char], i: usize) -> Option<usize> {
    if (i > 0 && latin_letter(chars.get(i - 1))) || !latin_letter(chars.get(i)) {
        return None;
    }
    let mut pairs = 0;
    while chars.get(i + 1 + 2 * pairs) == Some(&'.') && latin_letter(chars.get(i + 2 + 2 * pairs)) {
        pairs += 1;
    }
    (1..=pairs).rev().map(|pairs| i + 1 + 2 * pairs).find(|&end| !latin_letter(chars.get(end)))
}

/// Letters with dots: «L.A», «U.S.A»; a sentence's own dot stays.
fn dotted(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let Some(end) = dotted_at(chars, i) else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let letters: String = chars[i..end].iter().collect();
        if letter_or_digit(if i > 0 { chars.get(i - 1).copied() } else { None }) {
            out.push(' ');
        }
        out.extend(spell(&letters).chars());
        if letter_or_digit(chars.get(end).copied()) {
            out.push(' ');
        }
        i = end;
    }
    out
}

/// A Latin word at `i`: `[A-Za-z]+(?:['’][A-Za-z]+)*` — where it ends.
fn latin_word_at(chars: &[char], i: usize) -> Option<usize> {
    if !latin_letter(chars.get(i)) {
        return None;
    }
    let mut end = i;
    while latin_letter(chars.get(end)) {
        end += 1;
    }
    while matches!(chars.get(end), Some('\'' | '’')) && latin_letter(chars.get(end + 1)) {
        end += 1;
        while latin_letter(chars.get(end)) {
            end += 1;
        }
    }
    Some(end)
}

fn capitalized(said: &str) -> String {
    let mut chars = said.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn words(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let Some(end) = latin_word_at(chars, i) else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let latin: String = chars[i..end].iter().collect();
        let lower: String = latin.to_ascii_lowercase().chars().filter(|c| !matches!(c, '\'' | '’')).collect();
        // «BBC», «USA», «X», «U2»: letter by letter, unless a word is known («OK», «DJ», «I»).
        let abbreviation = latin == latin.to_ascii_uppercase() && latin.chars().count() <= 3 && known(&lower).is_none();
        let mut said = if abbreviation { spell(&latin) } else { word(&lower) };
        if !abbreviation && latin.chars().next() != lower.chars().next() {
            said = capitalized(&said);
        }
        // «U2» → «ю 2»: a number glued to the word gets its own word.
        if chars.get(end).is_some_and(char::is_ascii_digit) {
            said.push(' ');
        }
        if i > 0 && chars[i - 1].is_ascii_digit() {
            said.insert(0, ' ');
        }
        out.extend(said.chars());
        i = end;
    }
    out
}

/// The text with its Latin words in Russian letters, for an engine that reads only Cyrillic.
pub fn say_latin(text: &str) -> String {
    if !text.chars().any(|c| c.is_ascii_alphabetic()) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let said: String = words(&dotted(&decades(&slashes(&chars)))).into_iter().collect();
    let mut out = String::with_capacity(said.len());
    for c in said.chars() {
        if !(c == ' ' && out.ends_with(' ')) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_track_names_the_way_yandex_says_them() {
        assert_eq!(say_latin("Это Kavinsky, \"Nightcall 80's\""), "Это Кавински, \"Найткол восьмидесятых\"");
        assert_eq!(
            say_latin("Сейчас играет Daft Punk, \"One More Time\""),
            "Сейчас играет Дафт Панк, \"Уан Моур Тайм\""
        );
    }

    #[test]
    fn reads_words_by_english_rules() {
        assert_eq!(say_latin("Smells Like Teen Spirit"), "Смелс Лайк Тин Спирит");
        assert_eq!(say_latin("Highway to Hell"), "Хайуэй ту Хел");
        assert_eq!(say_latin("Queen, Metallica, Nirvana"), "Квин, Металика, Нирвана");
        assert_eq!(say_latin("girl, punk, music, time, sky"), "гёрл, панк, музик, тайм, скай");
    }

    #[test]
    fn spells_abbreviations_and_keeps_numbers_apart() {
        assert_eq!(say_latin("BBC и MTV"), "би би си и эм ти ви");
        assert_eq!(say_latin("Это AC/DC"), "Это эй си ди си");
        assert_eq!(say_latin("U2"), "ю 2");
        assert_eq!(say_latin("Malcolm X."), "Малколм экс.");
        assert_eq!(say_latin("Включаю Bluetooth."), "Включаю Блютуз.");
    }

    #[test]
    fn leaves_russian_text_as_it_is() {
        assert_eq!(say_latin("Включаю «Кино», 25 минут."), "Включаю «Кино», 25 минут.");
    }
}
