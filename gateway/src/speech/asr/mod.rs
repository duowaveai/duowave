//! Recognition: an OpenAI-compatible /audio/transcriptions, or the computer's own engine (local/), and the command
//! out of what it heard.

#[cfg(feature = "pc")]
pub mod local;
pub mod openai_transcriber;
pub mod speech_recognizer;
pub mod transcript;

use serde::{Deserialize, Serialize};

/// The language to recognise in: ASR_LANGUAGE when the .env sets it (blank — none: the model hears which for itself),
/// else the household's language (`crate::locale`), else none.
pub fn wanted_language(configured: Option<&str>, household: Option<String>) -> Option<String> {
    match configured.map(str::trim) {
        Some("") => None,
        Some(code) => Some(crate::locale::normalize(code).unwrap_or_else(|| code.to_string())),
        None => household,
    }
}

/// What a recognizer heard, with times: the text, the language when it tells one, and each word's start and end in
/// seconds from the audio's start (empty when the engine gives no times).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub text: String,
    pub language: Option<String>,
    pub words: Vec<HeardWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_s_language_comes_first_a_blank_one_is_none_and_unset_is_the_household_s() {
        assert_eq!(wanted_language(None, Some("en".into())).as_deref(), Some("en"));
        assert_eq!(wanted_language(None, None), None);
        assert_eq!(wanted_language(Some("ru"), Some("en".into())).as_deref(), Some("ru"));
        assert_eq!(wanted_language(Some(" en-GB "), Some("ru".into())).as_deref(), Some("en"));
        assert_eq!(wanted_language(Some(""), Some("en".into())), None);
    }
}
