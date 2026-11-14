//! Where a phrase came from: what opened it, on which device, by voice or in text. The fields are the ones the audio
//! interface keeps for every phrase (docs/plans/audio-interface.md, «Поля реплики»): this layer fills the computer's —
//! the button, the answer to a question, a typed text — and the audio interface adds the wake word, its name, the
//! character and who spoke.

use serde::{Deserialize, Serialize};

/// The device of a talk without a station: the journal's requests carry it, and the talk's state is kept under it.
pub const COMPUTER: &str = "computer";

/// What opened the phrase: the panel's «Сказать» (or its key), the answer to a question the assistant asked (the
/// microphone listened by itself), a typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activator {
    Button,
    Answer,
    Text,
}

impl Activator {
    pub fn parse(value: &str) -> Option<Activator> {
        match value {
            "button" => Some(Activator::Button),
            "answer" => Some(Activator::Answer),
            "text" => Some(Activator::Text),
            _ => None,
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
    pub device: String,
    pub via: Via,
}

impl Origin {
    /// A phrase of this computer: said after the button or after a question, or typed.
    pub fn computer(activator: Activator) -> Origin {
        let via = if activator == Activator::Text { Via::Text } else { Via::Voice };
        Origin { activator, device: COMPUTER.into(), via }
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
        assert_eq!(Activator::parse("word"), None, "the wake word comes with the audio interface");
    }
}
