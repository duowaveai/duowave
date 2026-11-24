//! Activators: an app on the station hears a word of its own and wakes the station as its button does; the request
//! starts with the sound before the word, so a phrase said in one breath — the word, then the command — reaches
//! recognition whole, and the gain is picked from the word itself (docs/plugins/AUTHORING.md, «Слово-активатор»).
//!
//! Two letters of the bus, each the owner's «да» once (`system/…`, bus/providers/system.rs):
//! - `system/mic` → `open`: where the station's processed microphone is — its `yio_audio` port and the channel. An
//!   address, not a lock: a program on the station could reach that port by itself; the letter makes the listening
//!   the owner's answer (a microphone's danger).
//! - `system/activator` → `wake` `{word, score, atMs, audio?, audioStartMs?}`: the word (an id of the app's
//!   `contributes.activators`), how sure the app is, when the word ended (ms since the epoch, the station's clock),
//!   and the sound around it — base64 of 16 kHz mono s16le, from `audioStartMs` to the moment of the letter.
//!
//! The station's gateway decides (StationSession::wake_by_activator): not while the station listens, nor right after
//! it was woken (its own wake word comes first); while it speaks, the word interrupts, as the button does. Otherwise
//! the station's Alice is asked to listen (aliced.rs) and her request, «activation directive», takes the sound before
//! the word in front of hers — asked once more when no request came.

pub mod aliced;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::ear::word::ActivatorSpec;

/// The sound of the station's microphone: 16 kHz mono.
pub const SAMPLE_RATE: i64 = 16_000;
const SAMPLES_PER_MS: i64 = SAMPLE_RATE / 1000;
/// What the request keeps before the word's end: the word itself and a breath before it. The station's own wake word
/// comes with about two seconds.
pub const BEFORE_WORD_MS: i64 = 1_500;
/// The sound an app may give with its letter, at most.
pub const AUDIO_MAX_MS: i64 = 4_000;
/// A word heard longer ago than this wakes nothing: the person has moved on.
pub const STALE_MS: i64 = 5_000;

/// A word heard: whose, which, how sure, when it ended, and the sound before it.
#[derive(Debug, Clone, PartialEq)]
pub struct Wake {
    pub app: String,
    pub word: String,
    /// The word as people say it.
    pub title: String,
    /// The word as recognition may write it: the command's text loses it.
    pub heard: Vec<String>,
    /// The character the request goes to, as the app's manifest names it.
    pub character: Option<String>,
    pub score: f64,
    pub at_ms: i64,
    /// The sound from `audio_start_ms` on: 16 kHz mono.
    pub audio: Vec<i16>,
    pub audio_start_ms: i64,
}

impl Wake {
    /// The sound the request starts with — from BEFORE_WORD_MS before the word's end to the end of what was given —
    /// and where in it the word ends (a sample). None without sound.
    pub fn preroll(&self) -> Option<(Vec<i16>, usize)> {
        if self.audio.is_empty() {
            return None;
        }
        let end = ((self.at_ms - self.audio_start_ms).max(0) * SAMPLES_PER_MS) as usize;
        let end = end.min(self.audio.len());
        let start = end.saturating_sub((BEFORE_WORD_MS * SAMPLES_PER_MS) as usize);
        Some((self.audio[start..].to_vec(), end - start))
    }

    /// The words the command's text loses: the word as said and as recognition may write it.
    pub fn wake_words(&self) -> Vec<String> {
        let mut words = vec![self.title.to_lowercase()];
        words.extend(self.heard.iter().cloned());
        words.dedup();
        words
    }
}

/// A request a word began: whose word, as said, as recognition may write it, and the character it goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub app: String,
    pub title: String,
    pub words: Vec<String>,
    pub character: Option<String>,
}

/// What came of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    /// The station listens: its request started with the sound before the word.
    Started,
    /// Not woken, and rightly so: why, in words (it listens already, it was woken just now).
    Skipped(String),
    /// Could not be woken: why.
    Failed(String),
}

impl WakeOutcome {
    /// The answer to the app's letter.
    pub fn answer(&self) -> Result<Value, String> {
        match self {
            WakeOutcome::Started => Ok(json!({ "started": true })),
            WakeOutcome::Skipped(reason) => Ok(json!({ "started": false, "reason": reason })),
            WakeOutcome::Failed(reason) => Err(reason.clone()),
        }
    }
}

/// Asks the station's Alice to listen once (aliced.rs, or a test's stand-in).
pub type Starter = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// The station this gateway runs on, as an activator reaches it (the session registry).
pub trait WakeStation: Send + Sync {
    fn wake(&self, wake: Wake, starter: Starter) -> BoxFuture<'static, WakeOutcome>;
}

/// The character a word switches to answering: the ear contract's (src/ear/word.rs).
pub use crate::ear::word::WordCharacter;

/// The activators' service: the station's microphone and its Alice, for the apps that hear their own words.
pub struct Activator {
    on_station: bool,
    ports: aliced::Ports,
    aliced: Option<SocketAddr>,
    station: Arc<dyn WakeStation>,
}

impl Activator {
    /// On the station, its own services by quasar.cfg; elsewhere none (the station's IPC answers on its 127.0.0.1).
    pub fn new(on_station: bool, station: Arc<dyn WakeStation>) -> Arc<Activator> {
        let ports = aliced::ports(Path::new(aliced::QUASAR_CFG));
        let aliced = on_station.then(|| SocketAddr::from(([127, 0, 0, 1], ports.aliced)));
        Arc::new(Activator { on_station, ports, aliced, station })
    }

    /// With Alice at an address of one's own: the tests.
    pub fn at(aliced: SocketAddr, station: Arc<dyn WakeStation>) -> Arc<Activator> {
        Arc::new(Activator { on_station: true, ports: aliced::Ports::default(), aliced: Some(aliced), station })
    }

    /// `system/mic` → `open`.
    pub fn mic(&self) -> Result<Value, String> {
        if !self.on_station {
            return Err(crate::tr!("микрофон колонки слышит только хаб на самой колонке"));
        }
        Ok(json!({ "port": self.ports.audio, "channel": aliced::MIC_CHANNEL, "rate": SAMPLE_RATE }))
    }

    /// `system/activator` → `wake`, for the app `app` with these words of its manifest.
    pub async fn wake(&self, app: &str, words: &[ActivatorSpec], input: &Value) -> Result<Value, String> {
        let Some(aliced) = self.aliced else {
            return Err(crate::tr!("будить колонку может только хаб на самой колонке"));
        };
        let wake = read_wake(app, words, input, crate::utils::time::now_ms() as i64)?;
        let starter: Starter = Arc::new(move || Box::pin(aliced::start_conversation(aliced)));
        let (word, score) = (wake.word.clone(), wake.score);
        let outcome = self.station.wake(wake, starter).await;
        tracing::info!(target: "plugins", "{app}: the word «{word}» ({score:.2}): {outcome:?}");
        outcome.answer()
    }
}

/// A `wake` letter read: the word must be one of the app's own, heard not long ago; its sound, when given, short and
/// whole.
pub fn read_wake(app: &str, words: &[ActivatorSpec], input: &Value, now: i64) -> Result<Wake, String> {
    let word = input.get("word").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if word.is_empty() {
        return Err(crate::tr!("нужно слово: word — id из contributes.activators"));
    }
    let Some(spec) = words.iter().find(|spec| spec.id == word) else {
        return Err(crate::tr!("слово «{word}» не объявлено в contributes.activators", word = word));
    };
    let Some(at_ms) = input.get("atMs").and_then(Value::as_i64) else {
        return Err(crate::tr!("atMs — когда слово кончилось, мс от 1970 года"));
    };
    let age = now - at_ms;
    if !(-STALE_MS..=STALE_MS).contains(&age) {
        return Err(crate::tr!("слово услышано {age} мс назад: поздно будить", age = age));
    }
    let score = input.get("score").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0);
    let (audio, audio_start_ms) = match input.get("audio").and_then(Value::as_str).filter(|audio| !audio.is_empty()) {
        None => (Vec::new(), at_ms),
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| crate::tr!("audio — base64 звука 16 кГц, моно, s16le"))?;
            if bytes.len() % 2 != 0 {
                return Err(crate::tr!("audio — base64 звука 16 кГц, моно, s16le"));
            }
            if bytes.len() as i64 > AUDIO_MAX_MS * SAMPLES_PER_MS * 2 {
                return Err(crate::tr!("звука больше {max} мс", max = AUDIO_MAX_MS));
            }
            let Some(start) = input.get("audioStartMs").and_then(Value::as_i64) else {
                return Err(crate::tr!("audioStartMs — когда начинается звук, мс от 1970 года"));
            };
            let length_ms = bytes.len() as i64 / 2 / SAMPLES_PER_MS;
            if start > at_ms || at_ms > start + length_ms + 100 {
                return Err(crate::tr!("конец слова (atMs) должен быть внутри звука"));
            }
            let (pairs, _) = bytes.as_chunks::<2>();
            (pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect(), start)
        }
    };
    Ok(Wake {
        app: app.to_string(),
        word,
        title: spec.title.clone(),
        heard: spec.heard.clone(),
        character: spec.character.clone(),
        score,
        at_ms,
        audio,
        audio_start_ms,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn word() -> ActivatorSpec {
        ActivatorSpec {
            id: "otto".into(),
            title: "Отто".into(),
            heard: vec!["ото".into()],
            character: Some("butler:otto".into()),
        }
    }

    fn encoded(samples: &[i16]) -> String {
        let bytes: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_word_of_its_own_with_its_sound_is_read() {
        let now = 1_000_000;
        // Two seconds of sound, the word ending 1.8 s into it.
        let samples: Vec<i16> = (0..32_000).map(|index| (index % 100) as i16).collect();
        let input = json!({ "word": "otto", "score": 0.97, "atMs": now - 200, "audio": encoded(&samples),
            "audioStartMs": now - 2000 });
        let wake = read_wake("words", &[word()], &input, now).unwrap();
        assert_eq!((wake.app.as_str(), wake.title.as_str(), wake.score), ("words", "Отто", 0.97));
        assert_eq!(wake.character.as_deref(), Some("butler:otto"));
        assert_eq!(wake.wake_words(), vec!["отто".to_string(), "ото".to_string()]);
        let (preroll, word_end) = wake.preroll().unwrap();
        // 1.5 s before the word's end, then the 0.2 s after it.
        assert_eq!((preroll.len(), word_end), (27_200, 24_000));
        assert_eq!(preroll[0], samples[4_800]);
    }

    #[test]
    fn other_words_old_words_and_broken_sound_are_refused() {
        let now = 1_000_000;
        let refused = |input: Value| read_wake("words", &[word()], &input, now).unwrap_err();
        assert!(refused(json!({ "atMs": now })).contains("нужно слово"));
        assert!(refused(json!({ "word": "max", "atMs": now })).contains("не объявлено"));
        assert!(refused(json!({ "word": "otto" })).contains("atMs"));
        assert!(refused(json!({ "word": "otto", "atMs": now - 9_000 })).contains("поздно"));
        assert!(refused(json!({ "word": "otto", "atMs": now, "audio": "%%%" })).contains("base64"));
        let long = encoded(&vec![0; 16 * 5_000]);
        assert!(refused(json!({ "word": "otto", "atMs": now, "audio": long, "audioStartMs": now - 5_000 }))
            .contains("больше"));
        let short = encoded(&vec![0; 16 * 500]);
        assert!(refused(json!({ "word": "otto", "atMs": now, "audio": short })).contains("audioStartMs"));
        assert!(refused(json!({ "word": "otto", "atMs": now, "audio": short, "audioStartMs": now - 3_000 }))
            .contains("внутри"));
        // Without sound: the station is still woken, only as by the button.
        let plain = read_wake("words", &[word()], &json!({ "word": "otto", "atMs": now }), now).unwrap();
        assert_eq!(plain.preroll(), None);
        assert_eq!(plain.score, 1.0);
    }

    struct Station(Mutex<Vec<Wake>>);

    impl WakeStation for Station {
        fn wake(&self, wake: Wake, starter: Starter) -> BoxFuture<'static, WakeOutcome> {
            self.0.lock().unwrap().push(wake);
            Box::pin(async move {
                match starter().await {
                    Ok(()) => WakeOutcome::Started,
                    Err(error) => WakeOutcome::Failed(error),
                }
            })
        }
    }

    #[tokio::test]
    async fn the_letter_wakes_the_station_through_its_alice() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let alice = tokio::spawn(async move { listener.accept().await.is_ok() });
        let station = Arc::new(Station(Mutex::new(Vec::new())));
        let activator = Activator::at(address, station.clone());
        let now = crate::utils::time::now_ms() as i64;
        let answer = activator.wake("words", &[word()], &json!({ "word": "otto", "atMs": now })).await.unwrap();
        assert_eq!(answer, json!({ "started": true }));
        assert!(alice.await.unwrap(), "Alice was asked");
        assert_eq!(station.0.lock().unwrap()[0].word, "otto");
        assert!(activator.mic().unwrap()["port"].as_u64().is_some());
    }

    #[tokio::test]
    async fn off_the_station_nothing_is_woken() {
        let station = Arc::new(Station(Mutex::new(Vec::new())));
        let activator = Activator::new(false, station.clone());
        let now = crate::utils::time::now_ms() as i64;
        let error = activator.wake("words", &[word()], &json!({ "word": "otto", "atMs": now })).await.unwrap_err();
        assert!(error.contains("только хаб на самой колонке"), "{error}");
        assert!(activator.mic().is_err());
        assert!(station.0.lock().unwrap().is_empty());
    }

    #[test]
    fn outcomes_answer_the_app() {
        assert_eq!(WakeOutcome::Skipped("слушает".into()).answer().unwrap()["reason"], "слушает");
        assert_eq!(WakeOutcome::Failed("нет".into()).answer().unwrap_err(), "нет");
    }
}
