//! Text helpers of the memory: the language of a record, words, and light stems that let «у Маши» find «Маша» and
//! «Barsik's» find «Barsik». Stage 1 needs only names and plain word matches; stage 2 brings Snowball stems for FTS5
//! (docs/plans/memory.md, «Поиск»).

/// The language of a record: picks the stems and is stored with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
    Other,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
            Lang::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Lang {
        match value {
            "ru" => Lang::Ru,
            "en" => Lang::En,
            _ => Lang::Other,
        }
    }
}

/// By the letters: more Cyrillic than Latin is Russian, Latin only is English.
pub fn lang_of(text: &str) -> Lang {
    let (mut cyrillic, mut latin) = (0usize, 0usize);
    for c in text.chars() {
        if ('\u{0400}'..='\u{04FF}').contains(&c) {
            cyrillic += 1;
        } else if c.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cyrillic == 0 && latin == 0 {
        Lang::Other
    } else if cyrillic >= latin {
        Lang::Ru
    } else {
        Lang::En
    }
}

/// Lower case, «ё» as «е», apostrophes kept inside words, everything else that is not a letter or a digit a space.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        let c = match c {
            'ё' | 'Ё' => 'е',
            '’' | '\'' => '\'',
            c => c,
        };
        if c.is_alphanumeric() || (c == '\'' && !space) {
            for lower in c.to_lowercase() {
                out.push(lower);
            }
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().trim_end_matches('\'').to_string()
}

/// The words of a text, normalized.
pub fn words(text: &str) -> Vec<String> {
    normalize(text).split(' ').filter(|w| !w.is_empty()).map(|w| w.trim_matches('\'').to_string()).collect()
}

const RU_ENDINGS: &[&str] = &[
    "иями", "ями", "ами", "ого", "его", "ому", "ему", "ыми", "ими", "ией", "ием", "иях", "ий", "ый", "ой", "ей", "ую",
    "юю", "ая", "яя", "ое", "ее", "ом", "ем", "ам", "ям", "ах", "ях", "ов", "ев", "ию", "ия", "ие", "ии", "ы", "и",
    "а", "я", "у", "ю", "е", "о", "ь", "й",
];

/// A light stem: the word without its case ending, so the forms of a name meet («маша», «маши», «машей» → «маш»).
/// It never cuts a word below three letters.
pub fn stem(word: &str) -> String {
    let word = word.trim_matches('\'');
    if word.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
        let len = word.chars().count();
        for ending in RU_ENDINGS {
            let cut = ending.chars().count();
            if len > cut + 2 && word.ends_with(ending) {
                return word.chars().take(len - cut).collect();
            }
        }
        return word.to_string();
    }
    let word = word.strip_suffix("'s").unwrap_or(word);
    let len = word.len();
    if len > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..len - 3]);
    }
    if len > 4 && (word.ends_with("ches") || word.ends_with("shes") || word.ends_with("sses") || word.ends_with("xes"))
    {
        return word[..len - 2].to_string();
    }
    if len > 3 && word.ends_with('s') && !word.ends_with("ss") {
        return word[..len - 1].to_string();
    }
    word.to_string()
}

/// The stems of a text's words.
pub fn stems(text: &str) -> Vec<String> {
    words(text).iter().map(|w| stem(w)).collect()
}

/// Whether a name (one word or several) is mentioned in the text, in any case form: «у Маши» mentions «Маша»,
/// «Анну Петровну» mentions «Анна Петровна».
pub fn mentions(text: &str, name: &str) -> bool {
    let name = stems(name);
    if name.is_empty() {
        return false;
    }
    let text = stems(text);
    text.windows(name.len()).any(|window| window == name.as_slice())
}

/// How much of the query is found in the text: the share of the query's stems that the text has (0.0–1.0).
pub fn overlap(query: &str, text: &str) -> f32 {
    let query: Vec<String> = stems(query).into_iter().filter(|s| !is_stop(s)).collect();
    if query.is_empty() {
        return 0.0;
    }
    let text = stems(text);
    let found = query.iter().filter(|q| text.contains(q)).count();
    found as f32 / query.len() as f32
}

/// Small words that say nothing about the subject.
fn is_stop(stem: &str) -> bool {
    matches!(
        stem,
        "про"
            | "об"
            | "о"
            | "в"
            | "на"
            | "у"
            | "с"
            | "и"
            | "что"
            | "это"
            | "мне"
            | "мой"
            | "моя"
            | "мое"
            | "the"
            | "a"
            | "an"
            | "of"
            | "about"
            | "that"
            | "my"
            | "is"
            | "to"
            | "and"
    )
}

/// The end of the original text that a normalized tail came from, in its own letters: the command words are cut,
/// «Маши» keeps its capital. The tail's words are the original's last words; end punctuation goes.
pub fn original_tail(original: &str, tail: &str) -> String {
    let want = words(tail).len();
    if want == 0 {
        return String::new();
    }
    let mut starts = Vec::new();
    let mut in_word = false;
    for (at, c) in original.char_indices() {
        let letter = c.is_alphanumeric();
        if letter && !in_word {
            starts.push(at);
        }
        in_word = letter || (in_word && matches!(c, '\'' | '’'));
    }
    if starts.len() < want {
        return tail.to_string();
    }
    let from = starts[starts.len() - want];
    original[from..].trim().trim_end_matches(['.', '!', '?', '…']).trim().to_string()
}

/// A rough token count for the prompt budget: about four characters a token, Cyrillic a bit denser.
pub fn tokens(text: &str) -> usize {
    let chars = text.chars().count();
    match lang_of(text) {
        Lang::Ru => chars.div_ceil(3),
        _ => chars.div_ceil(4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_language() {
        assert_eq!(lang_of("У Маши аллергия на орехи"), Lang::Ru);
        assert_eq!(lang_of("Masha is allergic to nuts"), Lang::En);
        assert_eq!(lang_of("12:30"), Lang::Other);
        assert_eq!(Lang::parse(Lang::En.as_str()), Lang::En);
    }

    #[test]
    fn normalizes_and_splits() {
        assert_eq!(normalize("  Запомни: у Маши — аллергия на орехи!  "), "запомни у маши аллергия на орехи");
        assert_eq!(normalize("Ёлка"), "елка");
        assert_eq!(words("Barsik's food, please"), vec!["barsik's", "food", "please"]);
    }

    #[test]
    fn stems_meet_across_cases() {
        for form in ["маша", "маши", "машу", "машей", "маше"] {
            assert_eq!(stem(form), "маш", "{form}");
        }
        for form in ["петя", "пети", "петей", "пете"] {
            assert_eq!(stem(form), "пет", "{form}");
        }
        assert_eq!(stem("барсика"), "барсик");
        assert_eq!(stem("кот"), "кот");
        assert_eq!(stem("barsik's"), "barsik");
        assert_eq!(stem("cats"), "cat");
        assert_eq!(stem("allergies"), "allergy");
        assert_eq!(stem("glass"), "glass");
    }

    #[test]
    fn finds_names_in_any_form() {
        assert!(mentions("у Маши аллергия на орехи", "Маша"));
        assert!(mentions("Подарок для Анны Петровны", "Анна Петровна"));
        assert!(!mentions("у Миши аллергия", "Маша"));
        assert!(mentions("feed Barsik's cat", "Barsik"));
        assert!(!mentions("anything", ""));
    }

    #[test]
    fn measures_overlap_without_small_words() {
        assert_eq!(overlap("про орехи", "У Маши аллергия на орехи"), 1.0);
        assert_eq!(overlap("орехи и молоко", "аллергия на орехи"), 0.5);
        assert_eq!(overlap("про", "что угодно"), 0.0);
    }

    #[test]
    fn keeps_the_original_letters_of_a_tail() {
        assert_eq!(
            original_tail("Запомни: у Маши аллергия на орехи!", "у маши аллергия на орехи"),
            "у Маши аллергия на орехи"
        );
        assert_eq!(
            original_tail("Remember that Masha's cat is Barsik.", "masha's cat is barsik"),
            "Masha's cat is Barsik"
        );
        assert_eq!(original_tail("запомни что-то", "что то"), "что-то");
        assert_eq!(original_tail("x", ""), "");
    }

    #[test]
    fn counts_tokens_roughly() {
        assert_eq!(tokens(""), 0);
        assert!(tokens("У Маши аллергия на орехи") >= 8);
        assert!(tokens("Masha is allergic to nuts") >= 6);
    }
}
