//! The recognition models this computer runs itself: GigaAM v3, Parakeet TDT v3 and Whisper as GGUF for
//! transcribe.cpp. The files are the ones Handy publishes (`handy-computer/*-gguf` on Hugging Face) at pinned
//! revisions, each with its size and SHA-256, in the quantization Handy picks (Q8_0). They are keyed by the names
//! asr-server and the panel's catalog already use (`v3_e2e_rnnt`, `parakeet-0.6b-v3`, …), so a choice made for the
//! Python server means the same model here.
//!
//! GigaAM's older generations and the multilingual GigaAM have no GGUF: they stay with an external recognition server.
//!
//! Languages are data here, not branches: each model lists the languages it knows (BCP 47 primary tags, as the
//! household's language is kept — `crate::locale::normalize`), and the list is in the order we advise the models. A
//! language's own model is the first that knows it ([`default_for`]); [`pick`] chooses what serves for the household's
//! language among the models that are here, and says which one to fetch when none of them knows it. A new language
//! needs no code: a model that knows it, in its place in the list.

/// One model file: where it comes from and how to know it arrived whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GgufModel {
    /// Our name for it: the catalog's `model` (showcase.json) and asr-server's.
    pub model: &'static str,
    /// Hugging Face repository and the revision the hash belongs to.
    pub repo: &'static str,
    pub revision: &'static str,
    pub file: &'static str,
    /// Bytes.
    pub size: u64,
    pub sha256: &'static str,
    /// The languages it knows. One: the model is always told it; more: the household's, when it is among them.
    pub languages: &'static [&'static str],
}

/// GigaAM v3: Russian only.
const RUSSIAN: &[&str] = &["ru"];

/// Parakeet TDT 0.6B v3: 25 European languages (NVIDIA's model card).
const PARAKEET: &[&str] = &[
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv", "mt", "nl", "pl", "pt",
    "ro", "ru", "sk", "sl", "sv", "uk",
];

/// Whisper: the languages of its tokenizer, as BCP 47 tags (Whisper's own `jw` for Javanese is `jv` here).
const WHISPER: &[&str] = &[
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy", "da", "de", "el", "en",
    "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw", "he", "hi", "hr", "ht", "hu", "hy", "id", "is",
    "it", "ja", "jv", "ka", "kk", "km", "kn", "ko", "la", "lb", "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn",
    "mr", "ms", "mt", "my", "ne", "nl", "nn", "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si", "sk",
    "sl", "sn", "so", "sq", "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz",
    "vi", "yi", "yo", "zh",
];

/// The model a new computer starts with: GigaAM v3 with punctuation, as asr-server's default. It also serves a
/// household whose language no model here knows yet — until the person fetches one that does.
pub const DEFAULT_MODEL: &str = "v3_e2e_rnnt";

/// In the order we advise them: a language's own model is the first one that knows it.
pub const MODELS: &[GgufModel] = &[
    GgufModel {
        model: "v3_e2e_rnnt",
        repo: "handy-computer/gigaam-v3-e2e-rnnt-gguf",
        revision: "f719d70812344f4d0fb8c11c0887b190501a7465",
        file: "gigaam-v3-e2e-rnnt-Q8_0.gguf",
        size: 273_724_832,
        sha256: "78d63b47723b7f8d78c6113a6ef983b5a86e2a86f6c273e1f5cb6967b1c4467a",
        languages: RUSSIAN,
    },
    GgufModel {
        model: "v3_e2e_ctc",
        repo: "handy-computer/gigaam-v3-e2e-ctc-gguf",
        revision: "075dff81f843cf23d22b4ce943ffdc4dd8650cd7",
        file: "gigaam-v3-e2e-ctc-Q8_0.gguf",
        size: 272_151_136,
        sha256: "9ccce4750dc813a493d96ca15ee251712bedec15ac9a02fa3d2bd732f08ae5eb",
        languages: RUSSIAN,
    },
    GgufModel {
        model: "v3_rnnt",
        repo: "handy-computer/gigaam-v3-rnnt-gguf",
        revision: "8f30356b4607ae2d79353ab7dc9e5eea6dc46b48",
        file: "gigaam-v3-rnnt-Q8_0.gguf",
        size: 273_022_880,
        sha256: "130d54450be2922144b265d90d453b781fc2b5adaee52a16954b00f3686201f7",
        languages: RUSSIAN,
    },
    GgufModel {
        model: "v3_ctc",
        repo: "handy-computer/gigaam-v3-ctc-gguf",
        revision: "c3c611444004820c21c3b68312a41c83c1e4813b",
        file: "gigaam-v3-ctc-Q8_0.gguf",
        size: 271_803_328,
        sha256: "71e5c82890e9e243a6bd7575f129f5d1bd2c3ca3ae79aab75cfb1a6934c6a62b",
        languages: RUSSIAN,
    },
    GgufModel {
        model: "parakeet-0.6b-v3",
        repo: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
        revision: "85ac09ea12fc4b1112fa76810059364bc6adc9de",
        file: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
        size: 739_508_576,
        sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
        languages: PARAKEET,
    },
    GgufModel {
        model: "whisper-large-v3-turbo",
        repo: "handy-computer/whisper-large-v3-turbo-gguf",
        revision: "5eaf945c7978e564bae5b28a5b1639dd93c2bfb1",
        file: "whisper-large-v3-turbo-Q8_0.gguf",
        size: 886_381_760,
        sha256: "b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183",
        languages: WHISPER,
    },
    GgufModel {
        model: "whisper-large-v3",
        repo: "handy-computer/whisper-large-v3-gguf",
        revision: "e3e29bee6389c7da4a141406f07bb80ddac5337c",
        file: "whisper-large-v3-Q8_0.gguf",
        size: 1_668_741_440,
        sha256: "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
        languages: WHISPER,
    },
    GgufModel {
        model: "whisper-medium",
        repo: "handy-computer/whisper-medium-gguf",
        revision: "ec78f06fded51aa82cde751678b78f76f78c8b7f",
        file: "whisper-medium-Q8_0.gguf",
        size: 831_538_144,
        sha256: "09e6a65e7de377aa5b10bae24608bc6f8ca2ed04b3993ef10d4a02bcd9a82adf",
        languages: WHISPER,
    },
    GgufModel {
        model: "whisper-small",
        repo: "handy-computer/whisper-small-gguf",
        revision: "c0214bd34be9296695486f838e0142f900803159",
        file: "whisper-small-Q8_0.gguf",
        size: 269_751_136,
        sha256: "9b9c8811bbcc82a7766f0fb0925614bdacb0923b2cc630daeac17108b655b860",
        languages: WHISPER,
    },
    GgufModel {
        model: "whisper-base",
        repo: "handy-computer/whisper-base-gguf",
        revision: "e0f69524f648720eca44c024d1d0dbb7027d1fa0",
        file: "whisper-base-Q8_0.gguf",
        size: 84_962_880,
        sha256: "81c069428bc8a24551a8169cf31cf09bcfd9d4cf50389ae281323c9aa9648c81",
        languages: WHISPER,
    },
    GgufModel {
        model: "whisper-tiny",
        repo: "handy-computer/whisper-tiny-gguf",
        revision: "6687f30c99641ee265df421e582354adbc8848fc",
        file: "whisper-tiny-Q8_0.gguf",
        size: 45_981_088,
        sha256: "325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834",
        languages: WHISPER,
    },
];

/// The model by our name; None: it has no GGUF (an external server's model).
pub fn find(model: &str) -> Option<&'static GgufModel> {
    MODELS.iter().find(|entry| entry.model == model)
}

/// The model a language starts with: the first in [`MODELS`] that knows it; None — no model here knows it.
pub fn default_for(language: &str) -> Option<&'static GgufModel> {
    MODELS.iter().find(|entry| entry.knows(language))
}

/// What serves, and what is missing for the household's language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// The model to serve (the catalog's name).
    pub model: &'static str,
    /// The language's own model, when nothing here knows the language: the person fetches it (nothing is fetched
    /// for them). None: the language is heard, or no model knows it.
    pub needs: Option<&'static str>,
}

/// The model to serve for a choice and a language.
///
/// - The person's choice (`asked`; none or not a model of ours — [`DEFAULT_MODEL`]) serves when it knows the
///   language, or when no language is wanted. It is fetched if it is not here: the person chose it.
/// - It does not know the language: a model that is here and knows it serves instead — the language's own model
///   first, then the others in the catalog's order. Nothing is fetched for that.
/// - None here knows it: the choice serves still, and `needs` names the model to fetch.
pub fn pick(asked: Option<&str>, language: Option<&str>, here: impl Fn(&GgufModel) -> bool) -> Pick {
    let chosen = asked.and_then(find).or_else(|| find(DEFAULT_MODEL)).expect("the default model is in the catalog");
    let Some(language) = language.filter(|language| !chosen.knows(language)) else {
        return Pick { model: chosen.model, needs: None };
    };
    let own = default_for(language);
    let present =
        own.filter(|own| here(own)).or_else(|| MODELS.iter().find(|entry| entry.knows(language) && here(entry)));
    match present {
        Some(entry) => Pick { model: entry.model, needs: None },
        None => Pick { model: chosen.model, needs: own.map(|own| own.model) },
    }
}

impl GgufModel {
    pub fn knows(&self, language: &str) -> bool {
        self.languages.contains(&language)
    }

    /// The language to tell it: its own when it knows only one; else the wanted one when it knows it; else none —
    /// the model hears which for itself.
    pub fn hint(&self, wanted: Option<&str>) -> Option<&'static str> {
        match self.languages {
            [only] => Some(only),
            languages => wanted.and_then(|wanted| languages.iter().copied().find(|known| *known == wanted)),
        }
    }

    /// Where to fetch it, in order: the mirror first when one is set (ASR_MODELS_MIRROR: a folder with the files by
    /// name), then Hugging Face at the pinned revision.
    pub fn urls(&self, mirror: Option<&str>) -> Vec<String> {
        let mut urls = Vec::new();
        if let Some(mirror) = mirror.map(str::trim).filter(|mirror| !mirror.is_empty()) {
            urls.push(format!("{}/{}", mirror.trim_end_matches('/'), self.file));
        }
        urls.push(format!("https://huggingface.co/{}/resolve/{}/{}", self.repo, self.revision, self.file));
        urls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_one_file_with_a_hash_and_a_size_and_the_default_is_among_them() {
        assert!(find(DEFAULT_MODEL).is_some());
        let mut names: Vec<&str> = MODELS.iter().map(|entry| entry.model).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), MODELS.len(), "a name twice");
        for entry in MODELS {
            assert_eq!(entry.sha256.len(), 64, "{}", entry.model);
            assert!(entry.sha256.chars().all(|c| c.is_ascii_hexdigit()), "{}", entry.model);
            assert_eq!(entry.revision.len(), 40, "{}", entry.model);
            assert!(entry.size > 10_000_000, "{}", entry.model);
            assert!(entry.file.ends_with(".gguf"), "{}", entry.model);
        }
    }

    #[test]
    fn gigaam_is_told_russian_and_the_multilingual_ones_the_wanted_language_and_old_gigaam_has_no_gguf() {
        let gigaam = find("v3_e2e_rnnt").unwrap();
        assert_eq!(gigaam.hint(None), Some("ru"));
        assert_eq!(gigaam.hint(Some("en")), Some("ru"));
        let parakeet = find("parakeet-0.6b-v3").unwrap();
        assert_eq!(parakeet.hint(None), None);
        assert_eq!(parakeet.hint(Some("en")), Some("en"));
        assert_eq!(parakeet.hint(Some("ru")), Some("ru"));
        // A language it does not know is not told: it would only mislead the model.
        assert_eq!(parakeet.hint(Some("ja")), None);
        assert_eq!(find("whisper-small").unwrap().hint(Some("ja")), Some("ja"));
        assert_eq!(find("v2_rnnt"), None);
        assert_eq!(find("multilingual_ctc"), None);
        assert_eq!(find("yandex"), None);
    }

    #[test]
    fn every_language_is_a_tag_as_the_household_keeps_it_and_a_language_has_its_own_model() {
        for entry in MODELS {
            assert!(!entry.languages.is_empty(), "{}", entry.model);
            for language in entry.languages {
                assert_eq!(crate::locale::normalize(language).as_deref(), Some(*language), "{}", entry.model);
            }
        }
        assert_eq!(default_for("ru").unwrap().model, DEFAULT_MODEL);
        assert_eq!(default_for("en").unwrap().model, "parakeet-0.6b-v3");
        assert_eq!(default_for("ja").unwrap().model, "whisper-large-v3-turbo");
        assert_eq!(default_for("xx"), None);
    }

    #[test]
    fn the_choice_serves_when_it_knows_the_language() {
        let nothing = |_: &GgufModel| false;
        assert_eq!(pick(None, None, nothing), Pick { model: DEFAULT_MODEL, needs: None });
        assert_eq!(pick(None, Some("ru"), nothing), Pick { model: DEFAULT_MODEL, needs: None });
        assert_eq!(pick(Some("v3_ctc"), Some("ru"), nothing), Pick { model: "v3_ctc", needs: None });
        // Chosen and not here yet: fetched, the person chose it.
        assert_eq!(pick(Some("whisper-small"), Some("en"), nothing), Pick { model: "whisper-small", needs: None });
        // A server's model is not ours: the default serves.
        assert_eq!(pick(Some("multilingual_ctc"), Some("ru"), nothing), Pick { model: DEFAULT_MODEL, needs: None });
    }

    #[test]
    fn a_one_language_choice_gives_way_to_a_model_here_that_knows_the_language() {
        let here = |models: &'static [&'static str]| move |entry: &GgufModel| models.contains(&entry.model);
        // English at home, GigaAM chosen: a Whisper that is here hears instead.
        assert_eq!(
            pick(Some("v3_e2e_rnnt"), Some("en"), here(&["v3_e2e_rnnt", "whisper-small"])),
            Pick { model: "whisper-small", needs: None }
        );
        // The language's own model first, whatever else is here.
        assert_eq!(
            pick(None, Some("en"), here(&["whisper-tiny", "parakeet-0.6b-v3"])),
            Pick { model: "parakeet-0.6b-v3", needs: None }
        );
        // Russian again: GigaAM, chosen, comes back.
        assert_eq!(
            pick(Some("v3_e2e_rnnt"), Some("ru"), here(&["v3_e2e_rnnt", "parakeet-0.6b-v3"])),
            Pick { model: "v3_e2e_rnnt", needs: None }
        );
    }

    #[test]
    fn no_model_here_knows_the_language_the_default_serves_and_the_needed_one_is_named_not_fetched() {
        let gigaam_only = |entry: &GgufModel| entry.model == DEFAULT_MODEL;
        assert_eq!(pick(None, Some("en"), gigaam_only), Pick { model: DEFAULT_MODEL, needs: Some("parakeet-0.6b-v3") });
        assert_eq!(
            pick(Some("v3_ctc"), Some("de"), gigaam_only),
            Pick { model: "v3_ctc", needs: Some("parakeet-0.6b-v3") }
        );
        // A language no model knows: nothing to name.
        assert_eq!(pick(None, Some("xx"), gigaam_only), Pick { model: DEFAULT_MODEL, needs: None });
    }

    #[test]
    fn a_mirror_comes_before_hugging_face_and_a_blank_one_is_no_mirror() {
        let model = find("v3_e2e_rnnt").unwrap();
        assert_eq!(
            model.urls(Some("https://updates.example.org/asr/")),
            vec![
                "https://updates.example.org/asr/gigaam-v3-e2e-rnnt-Q8_0.gguf".to_string(),
                "https://huggingface.co/handy-computer/gigaam-v3-e2e-rnnt-gguf/resolve/\
                 f719d70812344f4d0fb8c11c0887b190501a7465/gigaam-v3-e2e-rnnt-Q8_0.gguf"
                    .to_string(),
            ]
        );
        assert_eq!(model.urls(Some("  ")).len(), 1);
        assert_eq!(model.urls(None).len(), 1);
    }
}
