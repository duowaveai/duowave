//! What the character says back to a memory command, in the household's language and in the character's grammatical
//! gender («Запомнила» — Алиса, «Запомнил» — Артур). Short on purpose: the details are in the panel.

use chrono::{NaiveTime, Timelike};

use super::commands::PauseSpan;
use super::text::Lang;

/// How the answering character speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Speaker {
    pub lang: Lang,
    pub feminine: bool,
}

impl Speaker {
    fn en(self) -> bool {
        self.lang == Lang::En
    }

    /// The past tense of a Russian verb by gender: «запомнил» + «а».
    fn past(self, masculine: &str) -> String {
        if self.feminine {
            format!("{masculine}а")
        } else {
            masculine.to_string()
        }
    }
}

pub fn remembered(s: Speaker) -> String {
    if s.en() {
        "Got it, I'll remember.".into()
    } else {
        format!("{}.", capitalize(&s.past("запомнил")))
    }
}

/// «Запомни …» while the memory is paused.
pub fn paused_now(s: Speaker, until: Option<NaiveTime>) -> String {
    match (s.en(), until) {
        (true, Some(t)) => format!("I'm not remembering anything right now: memory is paused until {}.", clock(t)),
        (true, None) => "I'm not remembering anything right now: memory is paused.".into(),
        (false, Some(t)) => format!("Сейчас я ничего не запоминаю: память на паузе до {}.", clock(t)),
        (false, None) => "Сейчас я ничего не запоминаю: память на паузе.".into(),
    }
}

/// Any memory command while the memory is off.
pub fn memory_off(s: Speaker) -> String {
    if s.en() {
        "My memory is off. You can turn it on in the panel.".into()
    } else {
        "Память выключена. Включить её можно в пульте.".into()
    }
}

pub fn forgot(s: Speaker, fact: &str) -> String {
    let fact = fact.trim().trim_end_matches('.');
    if s.en() {
        format!("Okay, I've forgotten: {fact}.")
    } else {
        format!("Хорошо, {}: {fact}.", s.past("забыл"))
    }
}

pub fn nothing_to_forget(s: Speaker) -> String {
    if s.en() {
        "I don't remember anything like that.".into()
    } else {
        "Такого я не помню.".into()
    }
}

/// «Что ты помнишь про …?»: the facts as they are written, or that there are none.
pub fn recall(s: Speaker, facts: &[String]) -> String {
    if facts.is_empty() {
        return if s.en() {
            "I don't remember anything about that yet.".into()
        } else {
            "Об этом я пока ничего не помню.".into()
        };
    }
    let mut out = String::from(if s.en() { "Here's what I remember." } else { "Вот что я помню." });
    for fact in facts {
        out.push(' ');
        out.push_str(&capitalize(fact.trim().trim_end_matches('.')));
        out.push('.');
    }
    out
}

/// «Что ты обо мне знаешь?» when the speaker is not known: a voice at a station is anyone in the room.
pub fn who_are_you(s: Speaker) -> String {
    if s.en() {
        "I can't tell people apart by voice yet. Ask me about someone by name.".into()
    } else {
        "Я пока не различаю людей по голосу. Спросите про кого-нибудь по имени.".into()
    }
}

pub fn paused(s: Speaker, span: PauseSpan, until: Option<NaiveTime>) -> String {
    match (s.en(), span, until) {
        (true, PauseSpan::Hour, _) => "Okay. I won't remember anything for an hour.".into(),
        (true, PauseSpan::Morning, Some(t)) => format!("Okay. I won't remember anything until {}.", clock(t)),
        (true, _, _) => "Okay. I won't remember anything until you tell me.".into(),
        (false, PauseSpan::Hour, _) => "Хорошо. Час ничего не запоминаю.".into(),
        (false, PauseSpan::Morning, Some(t)) => format!("Хорошо. До {} ничего не запоминаю.", clock(t)),
        (false, _, _) => "Хорошо. Ничего не запоминаю, пока не скажете.".into(),
    }
}

pub fn resumed(s: Speaker) -> String {
    if s.en() {
        "Okay, I'm remembering again.".into()
    } else {
        "Хорошо, снова запоминаю.".into()
    }
}

/// Wiping, switching the memory off or reflection on: never by voice.
pub fn panel_only(s: Speaker) -> String {
    if s.en() {
        "That can only be done in the panel: Memory, then Settings.".into()
    } else {
        "Это можно сделать только в пульте: Память → Настройки.".into()
    }
}

/// «7:00», «18:30».
fn clock(t: NaiveTime) -> String {
    format!("{}:{:02}", t.hour(), t.minute())
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: Speaker = Speaker { lang: Lang::Ru, feminine: true };
    const ARTHUR: Speaker = Speaker { lang: Lang::Ru, feminine: false };
    const EN: Speaker = Speaker { lang: Lang::En, feminine: true };

    #[test]
    fn speaks_in_the_characters_gender() {
        assert_eq!(remembered(ALICE), "Запомнила.");
        assert_eq!(remembered(ARTHUR), "Запомнил.");
        // The fact keeps its words as written: it often starts with a name.
        assert_eq!(forgot(ALICE, "У Маши аллергия на орехи."), "Хорошо, забыла: У Маши аллергия на орехи.");
        assert_eq!(forgot(ARTHUR, "Петя ходит на шахматы"), "Хорошо, забыл: Петя ходит на шахматы.");
        assert_eq!(remembered(EN), "Got it, I'll remember.");
    }

    #[test]
    fn tells_the_pause() {
        let seven = NaiveTime::from_hms_opt(7, 0, 0).unwrap();
        assert_eq!(paused(ALICE, PauseSpan::Morning, Some(seven)), "Хорошо. До 7:00 ничего не запоминаю.");
        assert_eq!(paused(EN, PauseSpan::Hour, None), "Okay. I won't remember anything for an hour.");
        assert_eq!(paused_now(ALICE, Some(seven)), "Сейчас я ничего не запоминаю: память на паузе до 7:00.");
    }

    #[test]
    fn recalls_as_written() {
        let facts = vec!["У Маши аллергия на орехи".to_string(), "Маша работает в библиотеке.".to_string()];
        assert_eq!(recall(ALICE, &facts), "Вот что я помню. У Маши аллергия на орехи. Маша работает в библиотеке.");
        assert_eq!(recall(EN, &[]), "I don't remember anything about that yet.");
    }
}
