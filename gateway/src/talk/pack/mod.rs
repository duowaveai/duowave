//! The language packs of the talk: the words that depend on the language, one file per language. What is here now
//! is the assistant of a talk without a station — its name, who it is and how it speaks on a computer
//! (docs/plans/mic-mode.md, «Подсказка без колонки»). The audio interface adds the rules' words and the ready
//! phrases (docs/plans/audio-interface.md, «Правила работы и пакет языка»). A language without a pack takes the
//! Russian one: the household's first language.

mod en;
mod ru;

use crate::memory::text::Lang;

pub struct Pack {
    /// The built-in assistant's name where no station gives it one.
    pub name: &'static str,
    /// The built-in assistant's card in the panel.
    pub description: &'static str,
    /// Who the built-in assistant is: the first line of its prompt, before its regulators' lines.
    pub persona: &'static str,
    /// How any character speaks on a computer: every answer is said aloud, the microphone opens by the button.
    pub voice_rules: &'static [&'static str],
    /// What a computer has not: said after the rules.
    pub here: &'static str,
}

pub fn of(lang: Lang) -> &'static Pack {
    match lang {
        Lang::En => &en::PACK,
        Lang::Ru | Lang::Other => &ru::PACK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_names_the_assistant_and_none_names_a_station() {
        assert_eq!((of(Lang::Ru).name, of(Lang::En).name, of(Lang::Other).name), ("ДиВи", "Deevi", "ДиВи"));
        for pack in [of(Lang::Ru), of(Lang::En)] {
            let all = format!("{}\n{}\n{}\n{}", pack.description, pack.persona, pack.voice_rules.join("\n"), pack.here);
            for word in ["Алис", "колонк", "Яндекс", "Alice", "speaker", "station", "Yandex"] {
                assert!(!all.contains(word), "{word} in the pack of {}", pack.name);
            }
            assert!(pack.persona.contains(pack.name));
        }
    }
}
