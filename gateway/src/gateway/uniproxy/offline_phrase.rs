//! What the station says when it has no way out (operations/network_watch.rs) — at once, in the voice that answers,
//! recorded while it was online. No speech can be made without the network on the station (Alice's voice, the cloud's
//! and the Turbo's all need it), and a person waited about 10 s for «Не получилось получить ответ» or for silence (a
//! tester's Mini 2, 22.11: its Wi-Fi dropped for minutes many times a day).
//!
//! The phrase is recorded after an answer that was spoken — the network was there — when none is kept for the voice
//! that answers now, or its words changed (another language): one synthesis, an Ogg/Opus of a few tens of KB in the
//! state folder (`offline-phrase.ogg`, its voice and words beside it in `offline-phrase.json`). Nothing kept — the
//! request goes on as before.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// A voice whose recording failed is asked again after this long, not after every answer.
const RETRY: Duration = Duration::from_secs(10 * 60);
/// A recording bigger than this is no short phrase: not kept.
pub const BIGGEST: usize = 512 * 1024;
const AUDIO: &str = "offline-phrase.ogg";
const LABEL: &str = "offline-phrase.json";

/// Whose recording it is: the voice that said it and the words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Label {
    voice: String,
    text: String,
}

struct Keeper {
    dir: Mutex<Option<PathBuf>>,
    kept: Mutex<Option<(Label, Bytes)>>,
    tried: Mutex<Option<(Label, Instant)>>,
}

static KEEPER: OnceLock<Keeper> = OnceLock::new();

/// The words, in the household's language.
pub fn words() -> String {
    crate::tr!("У меня пропал интернет — вернусь, как только он появится.")
}

fn keeper() -> &'static Keeper {
    KEEPER.get_or_init(|| Keeper { dir: Mutex::new(None), kept: Mutex::new(None), tried: Mutex::new(None) })
}

/// The keeper with this state folder, and what it kept there before (at the start; before it, memory only).
pub fn init(state_dir: Option<&Path>) {
    let kept = state_dir.and_then(|dir| {
        let label: Label = serde_json::from_str(&std::fs::read_to_string(dir.join(LABEL)).ok()?).ok()?;
        let audio = std::fs::read(dir.join(AUDIO)).ok().filter(|audio| !audio.is_empty() && audio.len() <= BIGGEST)?;
        Some((label, Bytes::from(audio)))
    });
    let keeper = keeper();
    *keeper.dir.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = state_dir.map(Path::to_path_buf);
    *keeper.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = kept;
    *keeper.tried.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// The recording of the phrase in this voice, with today's words; None — not kept.
pub fn kept(voice: &str) -> Option<Bytes> {
    let text = words();
    let kept = keeper().kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.as_ref().filter(|(label, _)| label.voice == voice && label.text == text).map(|(_, audio)| audio.clone())
}

/// What to play with no way out: the recording kept, in whatever voice, with today's words. The voice that answers
/// may have changed with the outage — the Turbo went with the Wi-Fi, Alice's voice answers now (22.11: the phrase was
/// kept in the Turbo's voice, the station looked for one in Alice's and said nothing). The phrase in another voice is
/// better than silence.
pub fn to_play() -> Option<Bytes> {
    let text = words();
    let kept = keeper().kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.as_ref().filter(|(label, _)| label.text == text).map(|(_, audio)| audio.clone())
}

/// Whether the phrase is to be recorded in this voice now: none kept for it, and not tried lately. True takes the try.
pub fn wanted(voice: &str) -> bool {
    if kept(voice).is_some() {
        return false;
    }
    let label = Label { voice: voice.to_string(), text: words() };
    let mut tried = keeper().tried.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if tried.as_ref().is_some_and(|(was, at)| *was == label && at.elapsed() < RETRY) {
        return false;
    }
    *tried = Some((label, Instant::now()));
    true
}

/// Keeps a recording of the phrase (Ogg/Opus) said in this voice: in memory, and in the state folder.
pub fn keep(voice: &str, audio: Bytes) {
    if audio.is_empty() || audio.len() > BIGGEST {
        return;
    }
    let label = Label { voice: voice.to_string(), text: words() };
    let keeper = keeper();
    let dir = keeper.dir.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    if let Some(dir) = dir {
        // The audio first, the label last: a cut write leaves no label for the wrong audio.
        let written = write_whole(&dir.join(AUDIO), &audio)
            && serde_json::to_vec(&label).is_ok_and(|text| write_whole(&dir.join(LABEL), &text));
        if !written {
            tracing::warn!(target: "session", "the offline phrase could not be kept in the state folder");
        }
    }
    *keeper.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((label, audio));
}

/// A file written beside and moved over the old one.
fn write_whole(path: &Path, bytes: &[u8]) -> bool {
    let fresh = path.with_extension("new");
    std::fs::write(&fresh, bytes).is_ok() && std::fs::rename(&fresh, path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One process holds one keeper: the steps run in one test, in order.
    #[test]
    fn the_phrase_is_kept_per_voice_asked_once_in_a_while_and_read_back_after_a_restart() {
        let state = tempfile::tempdir().unwrap();
        init(Some(state.path()));
        let voice = "yandex:Алиса";
        assert_eq!(kept(voice), None);
        assert!(wanted(voice), "nothing kept: recorded after the next answer");
        assert!(!wanted(voice), "tried a moment ago: not after every answer");

        keep(voice, Bytes::from_static(b"OggS-pretend-opus"));
        assert_eq!(kept(voice).as_deref(), Some(&b"OggS-pretend-opus"[..]));
        assert_eq!(kept("own:ermil"), None, "another voice has its own recording");
        assert!(!wanted(voice));
        // With no way out the voice that answers may be another one: the kept phrase plays all the same.
        assert_eq!(to_play().as_deref(), Some(&b"OggS-pretend-opus"[..]));

        // Kept in the state folder: a new process reads it back.
        let label: Label = serde_json::from_slice(&std::fs::read(state.path().join(LABEL)).unwrap()).unwrap();
        assert_eq!(label, Label { voice: voice.into(), text: words() });
        assert_eq!(std::fs::read(state.path().join(AUDIO)).unwrap(), b"OggS-pretend-opus");

        // Too big or empty: not kept.
        keep("own:ermil", Bytes::from(vec![0u8; BIGGEST + 1]));
        keep("own:ermil", Bytes::new());
        assert_eq!(kept("own:ermil"), None);
    }
}
