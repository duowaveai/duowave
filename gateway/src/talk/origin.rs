//! Where a phrase came from: what opened it, on which device, by voice or in text — and the places kept for what comes
//! later: which wake word or button exactly, the character it was said to, who said it (docs/plans/audio-interface.md,
//! «Поля реплики»). A station's phrase is opened by its wake word (or is the answer to a question it asked); the
//! computer's by the button, the answer to a question, or a typed text. The record of a work, each of its notes and
//! the journal's request keep it.

use serde::{Deserialize, Serialize};

/// The device of a talk without a station: the journal's requests carry it, and the talk's state is kept under it.
pub const COMPUTER: &str = "computer";

/// The wake word of a Yandex station: today's only activator of a device. The prompts and the rules say the device's
/// activator, never this word itself.
pub const STATION_WORD: &str = "Алиса";

/// What opened the phrase: the device's wake word, the panel's «Сказать» (or its key), the answer to a question the
/// assistant asked (the microphone listened by itself), a typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activator {
    Word,
    Button,
    Answer,
    Text,
}

impl Activator {
    /// What a panel may say opened its phrase: the computer has no wake word yet.
    pub fn parse(value: &str) -> Option<Activator> {
        match value {
            "button" => Some(Activator::Button),
            "answer" => Some(Activator::Answer),
            "text" => Some(Activator::Text),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Activator::Word => "word",
            Activator::Button => "button",
            Activator::Answer => "answer",
            Activator::Text => "text",
        }
    }
}

/// By voice or in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Voice,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub activator: Activator,
    /// Which wake word or button exactly: a place for several on one device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The character it was said to (one per gateway today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// Who said it: a place for telling voices apart; the person signed in for the panel's own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    pub device: String,
    pub via: Via,
}

impl Origin {
    /// A phrase of this computer: said after the button or after a question, or typed.
    pub fn computer(activator: Activator) -> Origin {
        let via = if activator == Activator::Text { Via::Text } else { Via::Voice };
        Origin { activator, name: None, character: None, who: None, device: COMPUTER.into(), via }
    }

    /// A station's phrase: with its wake word (which one: today's only, `STATION_WORD`), or without it — the answer
    /// to a question the station listened for.
    pub fn station(device: &str, wake_word: bool) -> Origin {
        let (activator, name) =
            if wake_word { (Activator::Word, Some(STATION_WORD.to_string())) } else { (Activator::Answer, None) };
        Origin { activator, name, character: None, who: None, device: device.to_string(), via: Via::Voice }
    }

    /// Who said it.
    pub fn by(mut self, who: Option<String>) -> Origin {
        self.who = who.filter(|who| !who.is_empty());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_phrase_is_text_and_the_rest_is_voice() {
        assert_eq!(Origin::computer(Activator::Text).via, Via::Text);
        assert_eq!(Origin::computer(Activator::Button).via, Via::Voice);
        assert_eq!(Origin::computer(Activator::Answer).via, Via::Voice);
        assert_eq!(
            serde_json::to_value(Origin::computer(Activator::Answer)).unwrap(),
            serde_json::json!({"activator": "answer", "device": "computer", "via": "voice"})
        );
        assert_eq!(Activator::parse("word"), None, "the computer has no wake word yet");
    }

    #[test]
    fn a_station_phrase_is_opened_by_its_wake_word_or_answers_a_question() {
        let woken = Origin::station("kitchen", true).by(Some("Голос дома".into()));
        assert_eq!(
            serde_json::to_value(&woken).unwrap(),
            serde_json::json!({"activator": "word", "name": "Алиса", "who": "Голос дома", "device": "kitchen", "via": "voice"})
        );
        assert_eq!(Origin::station("kitchen", false).activator, Activator::Answer);
        let read: Origin =
            serde_json::from_value(serde_json::json!({"activator": "word", "device": "hall", "via": "voice"})).unwrap();
        assert_eq!((read.name, read.character, read.who), (None, None, None), "the places are empty when unknown");
    }
}
