//! The recognition models this computer runs itself: GigaAM v3, Parakeet TDT v3 and Whisper as GGUF for
//! transcribe.cpp. The files are the ones Handy publishes (`handy-computer/*-gguf` on Hugging Face) at pinned
//! revisions, each with its size and SHA-256, in the quantization Handy picks (Q8_0). They are keyed by the names
//! asr-server and the panel's catalog already use (`v3_e2e_rnnt`, `parakeet-0.6b-v3`, …), so a choice made for the
//! Python server means the same model here.
//!
//! GigaAM's older generations and the multilingual GigaAM have no GGUF: they stay with an external recognition server.

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
    /// The language to tell the model, when it is a one-language model; None: the gateway's ASR_LANGUAGE, if any.
    pub language: Option<&'static str>,
}

/// The model a new computer starts with: GigaAM v3 with punctuation, as asr-server's default.
pub const DEFAULT_MODEL: &str = "v3_e2e_rnnt";

pub const MODELS: &[GgufModel] = &[
    GgufModel {
        model: "v3_e2e_rnnt",
        repo: "handy-computer/gigaam-v3-e2e-rnnt-gguf",
        revision: "f719d70812344f4d0fb8c11c0887b190501a7465",
        file: "gigaam-v3-e2e-rnnt-Q8_0.gguf",
        size: 273_724_832,
        sha256: "78d63b47723b7f8d78c6113a6ef983b5a86e2a86f6c273e1f5cb6967b1c4467a",
        language: Some("ru"),
    },
    GgufModel {
        model: "v3_e2e_ctc",
        repo: "handy-computer/gigaam-v3-e2e-ctc-gguf",
        revision: "075dff81f843cf23d22b4ce943ffdc4dd8650cd7",
        file: "gigaam-v3-e2e-ctc-Q8_0.gguf",
        size: 272_151_136,
        sha256: "9ccce4750dc813a493d96ca15ee251712bedec15ac9a02fa3d2bd732f08ae5eb",
        language: Some("ru"),
    },
    GgufModel {
        model: "v3_rnnt",
        repo: "handy-computer/gigaam-v3-rnnt-gguf",
        revision: "8f30356b4607ae2d79353ab7dc9e5eea6dc46b48",
        file: "gigaam-v3-rnnt-Q8_0.gguf",
        size: 273_022_880,
        sha256: "130d54450be2922144b265d90d453b781fc2b5adaee52a16954b00f3686201f7",
        language: Some("ru"),
    },
    GgufModel {
        model: "v3_ctc",
        repo: "handy-computer/gigaam-v3-ctc-gguf",
        revision: "c3c611444004820c21c3b68312a41c83c1e4813b",
        file: "gigaam-v3-ctc-Q8_0.gguf",
        size: 271_803_328,
        sha256: "71e5c82890e9e243a6bd7575f129f5d1bd2c3ca3ae79aab75cfb1a6934c6a62b",
        language: Some("ru"),
    },
    GgufModel {
        model: "parakeet-0.6b-v3",
        repo: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
        revision: "85ac09ea12fc4b1112fa76810059364bc6adc9de",
        file: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
        size: 739_508_576,
        sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
        language: None,
    },
    GgufModel {
        model: "whisper-large-v3-turbo",
        repo: "handy-computer/whisper-large-v3-turbo-gguf",
        revision: "5eaf945c7978e564bae5b28a5b1639dd93c2bfb1",
        file: "whisper-large-v3-turbo-Q8_0.gguf",
        size: 886_381_760,
        sha256: "b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183",
        language: None,
    },
    GgufModel {
        model: "whisper-large-v3",
        repo: "handy-computer/whisper-large-v3-gguf",
        revision: "e3e29bee6389c7da4a141406f07bb80ddac5337c",
        file: "whisper-large-v3-Q8_0.gguf",
        size: 1_668_741_440,
        sha256: "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
        language: None,
    },
    GgufModel {
        model: "whisper-medium",
        repo: "handy-computer/whisper-medium-gguf",
        revision: "ec78f06fded51aa82cde751678b78f76f78c8b7f",
        file: "whisper-medium-Q8_0.gguf",
        size: 831_538_144,
        sha256: "09e6a65e7de377aa5b10bae24608bc6f8ca2ed04b3993ef10d4a02bcd9a82adf",
        language: None,
    },
    GgufModel {
        model: "whisper-small",
        repo: "handy-computer/whisper-small-gguf",
        revision: "c0214bd34be9296695486f838e0142f900803159",
        file: "whisper-small-Q8_0.gguf",
        size: 269_751_136,
        sha256: "9b9c8811bbcc82a7766f0fb0925614bdacb0923b2cc630daeac17108b655b860",
        language: None,
    },
    GgufModel {
        model: "whisper-base",
        repo: "handy-computer/whisper-base-gguf",
        revision: "e0f69524f648720eca44c024d1d0dbb7027d1fa0",
        file: "whisper-base-Q8_0.gguf",
        size: 84_962_880,
        sha256: "81c069428bc8a24551a8169cf31cf09bcfd9d4cf50389ae281323c9aa9648c81",
        language: None,
    },
    GgufModel {
        model: "whisper-tiny",
        repo: "handy-computer/whisper-tiny-gguf",
        revision: "6687f30c99641ee265df421e582354adbc8848fc",
        file: "whisper-tiny-Q8_0.gguf",
        size: 45_981_088,
        sha256: "325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834",
        language: None,
    },
];

/// The model by our name; None: it has no GGUF (an external server's model).
pub fn find(model: &str) -> Option<&'static GgufModel> {
    MODELS.iter().find(|entry| entry.model == model)
}

impl GgufModel {
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
    fn gigaam_is_told_russian_and_the_multilingual_ones_are_not_and_old_gigaam_has_no_gguf() {
        assert_eq!(find("v3_e2e_rnnt").unwrap().language, Some("ru"));
        assert_eq!(find("parakeet-0.6b-v3").unwrap().language, None);
        assert_eq!(find("v2_rnnt"), None);
        assert_eq!(find("multilingual_ctc"), None);
        assert_eq!(find("yandex"), None);
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
