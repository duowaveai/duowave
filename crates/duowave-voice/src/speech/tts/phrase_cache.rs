//! A store of ready phrases in the household's own voice (docs/plans/audio-interface.md, «Готовые фразы своим
//! голосом»): the language pack's constant phrases («Делаю.», «Добавлю.», «На столе.», «Открыто в пульте.»…) and a
//! work's «…: готово.» are synthesized ahead — in the background, when the voice is chosen and at the start — so the
//! first sound of the layer's line comes at once instead of after the voice server's seconds. Only our own voice:
//! Alice's answers in a third of a second anyway.
//!
//! On disk, the key is the voice and the text; both kinds of sound are kept — Ogg/Opus for a station, WAV for a
//! computer. At most 20 MB: the phrases used least lately go first.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bytes::Bytes;
use sha2::{Digest, Sha256};

/// The store at most this big.
pub const LIMIT_BYTES: u64 = 20 * 1024 * 1024;

/// What kind of sound a phrase is kept as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Ogg/Opus, as a station plays it.
    Opus,
    /// WAV, as a computer's speakers do.
    Wav,
}

impl Sound {
    fn extension(self) -> &'static str {
        match self {
            Sound::Opus => "ogg",
            Sound::Wav => "wav",
        }
    }
}

pub struct PhraseCache {
    dir: PathBuf,
    limit: u64,
    /// The texts kept ahead for each voice: a phrase that begins with one of them starts with its sound.
    known: Mutex<BTreeSet<(String, String)>>,
}

/// The stem a phrase is recognized by at the start of a longer one: «Делаю.» → «Делаю».
fn stem(text: &str) -> &str {
    text.trim().trim_end_matches(['.', '!', '?', '…']).trim_end()
}

impl PhraseCache {
    pub fn new(dir: PathBuf) -> PhraseCache {
        PhraseCache { dir, limit: LIMIT_BYTES, known: Mutex::new(BTreeSet::new()) }
    }

    #[cfg(test)]
    fn with_limit(dir: PathBuf, limit: u64) -> PhraseCache {
        PhraseCache { dir, limit, known: Mutex::new(BTreeSet::new()) }
    }

    fn path(&self, voice: &str, sound: Sound, text: &str) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(voice.as_bytes());
        hash.update([0]);
        hash.update(text.trim().as_bytes());
        self.dir.join(format!("{}.{}", hex::encode(hash.finalize()), sound.extension()))
    }

    /// The phrase's sound, kept for this voice; reading it makes it the newest.
    pub fn get(&self, voice: &str, sound: Sound, text: &str) -> Option<Bytes> {
        let path = self.path(voice, sound, text);
        let bytes = std::fs::read(&path).ok()?;
        let _ = filetime_now(&path);
        Some(Bytes::from(bytes))
    }

    /// Keeps a phrase's sound for this voice; the oldest go while the store is over its size.
    pub fn put(&self, voice: &str, sound: Sound, text: &str, bytes: &[u8]) {
        if bytes.is_empty() || std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.path(voice, sound, text);
        if let Err(error) = std::fs::write(&path, bytes) {
            tracing::warn!(target: "tts", "a ready phrase is not kept: {error}");
            return;
        }
        self.known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((voice.to_string(), text.trim().to_string()));
        self.evict();
    }

    /// A phrase this voice keeps already (from before a restart): a longer one may begin with it.
    pub fn note(&self, voice: &str, text: &str) {
        self.known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((voice.to_string(), text.trim().to_string()));
    }

    /// Whether this voice keeps the phrase as both kinds of sound.
    pub fn has(&self, voice: &str, text: &str) -> bool {
        [Sound::Opus, Sound::Wav].iter().all(|sound| self.path(voice, *sound, text).is_file())
    }

    /// A phrase that begins with a kept one: its sound and what is left to synthesize («Делаю: испанские слова дня.» →
    /// «Делаю.» kept, «испанские слова дня.» left). The longest kept one wins; the whole phrase kept leaves nothing.
    pub fn head(&self, voice: &str, sound: Sound, text: &str) -> Option<(Bytes, String)> {
        let text = text.trim();
        let known: Vec<String> = self
            .known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(whose, _)| whose == voice)
            .map(|(_, known)| known.clone())
            .collect();
        let mut best: Option<(String, String)> = None;
        for known in known {
            let rest = if text == known {
                String::new()
            } else {
                let stem = stem(&known);
                let Some(after) = text.strip_prefix(stem) else { continue };
                // Only at a word's end: «Делаю:» and «Делаю.» begin with «Делаю», «Делаюсь» does not.
                if !after.starts_with([':', '.', ',', ' ', '!', '?']) {
                    continue;
                }
                after.trim_start_matches([':', '.', ',', '!', '?']).trim_start().to_string()
            };
            if best.as_ref().is_none_or(|(kept, _)| known.len() > kept.len()) {
                best = Some((known, rest));
            }
        }
        let (known, rest) = best?;
        Some((self.get(voice, sound, &known)?, rest))
    }

    /// The store kept under its size: the files read or written longest ago go first.
    fn evict(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                meta.is_file().then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len(), entry.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
        files.sort();
        for (_, size, path) in files {
            if total <= self.limit {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

/// A file read now counts as new: its time set to now.
fn filetime_now(path: &Path) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.set_modified(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_voices_phrases_apart_by_voice_and_sound_and_starts_a_longer_one_with_a_kept_one() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PhraseCache::new(dir.path().join("phrases"));
        assert_eq!(cache.get("silero/xenia", Sound::Opus, "Делаю."), None);
        cache.put("silero/xenia", Sound::Opus, "Делаю.", b"ogg-delayu");
        cache.put("silero/xenia", Sound::Wav, "Делаю.", b"wav-delayu");
        cache.put("silero/xenia", Sound::Opus, "Часы: готово.", b"ogg-gotovo");
        assert!(cache.has("silero/xenia", "Делаю.") && !cache.has("silero/xenia", "Часы: готово."));
        assert_eq!(cache.get("silero/xenia", Sound::Wav, "Делаю.").as_deref(), Some(&b"wav-delayu"[..]));
        assert_eq!(cache.get("piper/irina", Sound::Opus, "Делаю."), None, "another voice keeps its own");

        let head =
            |text: &str| cache.head("silero/xenia", Sound::Opus, text).map(|(bytes, rest)| (bytes.to_vec(), rest));
        assert_eq!(head("Делаю: испанские слова дня."), Some((b"ogg-delayu".to_vec(), "испанские слова дня.".into())));
        assert_eq!(head("Делаю."), Some((b"ogg-delayu".to_vec(), String::new())));
        assert_eq!(
            head("Часы: готово. На стол или примерить?"),
            Some((b"ogg-gotovo".to_vec(), "На стол или примерить?".into()))
        );
        assert_eq!(head("Делаюсь как могу."), None, "only at a word's end");
        assert_eq!(head("Сейчас посмотрю."), None);
        assert_eq!(cache.head("silero/xenia", Sound::Wav, "Часы: готово."), None, "kept as Ogg only");
    }

    #[test]
    fn the_phrases_used_least_lately_go_first_when_the_store_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PhraseCache::with_limit(dir.path().to_path_buf(), 25);
        cache.put("v", Sound::Opus, "раз", &[1; 10]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.put("v", Sound::Opus, "два", &[2; 10]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.get("v", Sound::Opus, "раз").is_some(), "read now: the newest");
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.put("v", Sound::Opus, "три", &[3; 10]);
        assert!(cache.get("v", Sound::Opus, "два").is_none(), "used least lately: gone");
        assert!(cache.get("v", Sound::Opus, "раз").is_some() && cache.get("v", Sound::Opus, "три").is_some());
    }
}
