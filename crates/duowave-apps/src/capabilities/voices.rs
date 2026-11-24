//! Voices for apps (`system/voice` → `synth`, `voices`, `providers`): one letter, providers behind it. The computer's
//! own voices (Piper, Silero and the rest through the voice server) and ElevenLabs by the owner's key in the gateway's
//! safe; a new provider is one more `VoiceProvider` registered in app.rs, with no change in any app. A voice's id names
//! its provider — `elevenlabs/<id>`; any other id is the computer's `<engine>/<voice>`.
//!
//! A voice that sends the text out of the house or spends money (cloud, paid) speaks for an app only after the owner's
//! «да» for `system/voice/cloud` (links/plugins.rs asks the bus). The provider's word times come back with the sound;
//! where it has none, the gateway's hearing finds them (capabilities/hearing.rs).
//!
//! The regulator «Что может уходить из дома» (docs/plans/home-and-helpers.md) governs them too: a provider whose text
//! would leave the house — ElevenLabs, the computer's voice server at a cloud address — is not ready and voices nothing
//! while the regulator keeps the clouds on keys in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::core::helpers::Reach;
use crate::operations::networks::is_local_url;
use crate::speech::asr::HeardWord;

/// Whether the regulator «Что может уходить из дома» lets a road of this reach out now (the helpers' `lets`).
pub type Gate = Arc<dyn Fn(Reach) -> bool + Send + Sync>;

/// The computer's provider: its voices are `<engine>/<voice>`.
pub const COMPUTER: &str = "computer";
/// The address an app needs the owner's «да» for before a cloud or paid voice speaks for it.
pub const CLOUD: &str = "system/voice/cloud";

/// A provider as the panel and the apps see it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderCard {
    pub id: &'static str,
    pub title: String,
    /// Can voice now: the voice server is set up, the key is in the safe.
    pub ready: bool,
    /// What it needs before it is ready, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    /// The text leaves the house for the provider's servers.
    pub cloud: bool,
    /// Voicing spends the owner's money (the provider's plan).
    pub paid: bool,
    /// Gives each word's time itself; otherwise the gateway's hearing finds them.
    pub words: bool,
    /// The key as the panel may show it — «задан · …a1b2», never the key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<crate::secrets::SecretMask>,
    pub about: String,
    /// Why its voices did not come last time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A voiced line: the sound, its file kind and the times the provider knows.
pub struct Spoken {
    pub bytes: Vec<u8>,
    /// The file's extension: `wav`, `mp3`.
    pub ext: &'static str,
    /// Each character's time as the provider gives it: `{characters, character_start_times_seconds,
    /// character_end_times_seconds}` (ElevenLabs).
    pub alignment: Option<Value>,
    /// Each word's time, seconds from the start.
    pub words: Vec<HeardWord>,
}

/// One line to voice, as an app asks it.
#[derive(Debug, Clone, Default)]
pub struct Line {
    /// The provider's own id of the voice: `piper/denis` for the computer, the voice's id for ElevenLabs.
    pub voice: String,
    pub text: String,
    /// A language hint: `ru`, `en`.
    pub lang: Option<String>,
    /// The lines around it, for a provider that keeps the intonation from line to line (ElevenLabs).
    pub previous: Option<String>,
    pub next: Option<String>,
}

/// One provider of voices.
pub trait VoiceProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn card(&self) -> ProviderCard;
    /// Its voices as apps see them: `{id, title, provider, license, publish, cloud, paid}`.
    fn voices(&self) -> BoxFuture<'static, Result<Vec<Value>, String>>;
    /// Voices one line.
    fn synth(&self, line: Line, cancel: CancellationToken) -> BoxFuture<'static, Result<Spoken, String>>;
    /// What is left on the provider's plan: `{used, limit, tier}` in its units; none when it has no plan.
    fn balance(&self) -> BoxFuture<'static, Result<Option<Value>, String>> {
        Box::pin(async { Ok(None) })
    }
    /// The owner's key for it, typed in the panel; none removes it.
    fn set_key(&self, key: Option<&str>) -> Result<(), String> {
        let _ = key;
        Err(crate::tr!("этому поставщику ключ не нужен"))
    }
    /// How far an app's text goes with it now: home (the computer's server at home), or out of it (a cloud).
    fn reach(&self) -> Reach {
        Reach::Home
    }
}

/// The registry: who voices, and which provider a voice belongs to.
pub struct Voices {
    list: RwLock<Vec<Arc<Governed>>>,
    /// The last failure of each provider's list of voices, for its card.
    problems: Mutex<HashMap<&'static str, String>>,
    /// The regulator over the providers whose text leaves the house.
    gate: Gate,
}

/// What a provider says while the regulator keeps its road in.
fn closed() -> String {
    crate::tr!("Регулятор «Что может уходить из дома» не пускает облака по ключам.")
}

/// A provider as the registry gives it out: under the regulator — one whose text would leave the house while the
/// regulator keeps it in is not ready, lists no voices and voices nothing.
struct Governed {
    inner: Arc<dyn VoiceProvider>,
    gate: Gate,
}

impl Governed {
    fn shut(&self) -> bool {
        let reach = self.inner.reach();
        reach != Reach::Home && !(self.gate)(reach)
    }

    /// It would voice but for the regulator: shut, and ready by itself (a key, a server).
    fn kept_in(&self) -> bool {
        self.shut() && self.inner.card().ready
    }
}

impl VoiceProvider for Governed {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn card(&self) -> ProviderCard {
        let mut card = self.inner.card();
        if self.shut() {
            card.ready = false;
            card.needs = Some(closed());
        }
        card
    }

    fn voices(&self) -> BoxFuture<'static, Result<Vec<Value>, String>> {
        if self.shut() {
            return Box::pin(async { Err(closed()) });
        }
        self.inner.voices()
    }

    fn synth(&self, line: Line, cancel: CancellationToken) -> BoxFuture<'static, Result<Spoken, String>> {
        if self.shut() {
            return Box::pin(async { Err(closed()) });
        }
        self.inner.synth(line, cancel)
    }

    fn balance(&self) -> BoxFuture<'static, Result<Option<Value>, String>> {
        self.inner.balance()
    }

    fn set_key(&self, key: Option<&str>) -> Result<(), String> {
        self.inner.set_key(key)
    }

    fn reach(&self) -> Reach {
        self.inner.reach()
    }
}

impl Voices {
    /// The registry under the regulator: `gate` says whether a road of a reach may be taken now.
    pub fn new(gate: Gate) -> Voices {
        Voices { list: RwLock::new(Vec::new()), problems: Mutex::new(HashMap::new()), gate }
    }

    /// A provider voices from now on, under the regulator; one with the same id goes.
    pub fn register(&self, provider: Arc<dyn VoiceProvider>) {
        let provider = Arc::new(Governed { inner: provider, gate: self.gate.clone() });
        let mut list = self.list.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        list.retain(|known| known.id() != provider.id());
        list.push(provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn VoiceProvider>> {
        let list = self.list.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        list.iter().find(|one| one.id() == id).map(|one| one.clone() as Arc<dyn VoiceProvider>)
    }

    fn providers(&self) -> Vec<Arc<Governed>> {
        self.list.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn cards(&self) -> Vec<ProviderCard> {
        let problems = self.problems.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        self.providers()
            .iter()
            .map(|provider| {
                let mut card = provider.card();
                if card.ready {
                    card.problem = problems.get(card.id).cloned();
                }
                card
            })
            .collect()
    }

    /// Which provider voices `voice`, and that provider's own id of it: `elevenlabs/<id>` (or the older
    /// `elevenlabs:<id>`) is ElevenLabs'; any other id is the computer's.
    pub fn route(&self, voice: &str) -> Option<(Arc<dyn VoiceProvider>, String)> {
        if let Some(at) = voice.find(['/', ':']) {
            let (prefix, rest) = (&voice[..at], &voice[at + 1..]);
            if prefix != COMPUTER && !rest.is_empty() {
                if let Some(provider) = self.get(prefix) {
                    return Some((provider, rest.to_string()));
                }
            }
        }
        self.get(COMPUTER).map(|provider| (provider, voice.to_string()))
    }

    /// The voices of every ready provider, one after another; a provider that fails gives none, and its card says why.
    /// No provider ready at all — an error, so an app knows to go on without a voice: the regulator's words when one
    /// would voice but for it.
    pub async fn all(&self) -> Result<Vec<Value>, String> {
        let providers = self.providers();
        let ready: Vec<_> = providers.iter().filter(|provider| provider.card().ready).cloned().collect();
        if ready.is_empty() {
            if providers.iter().any(|provider| provider.kept_in()) {
                return Err(closed());
            }
            return Err(crate::tr!("голосов нет: сервер голоса не настроен, ключей облачных голосов нет"));
        }
        let mut voices = Vec::new();
        let mut failed = Vec::new();
        for provider in ready {
            let id = provider.id();
            match provider.voices().await {
                Ok(list) => {
                    self.problems.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
                    voices.extend(list);
                }
                Err(error) => {
                    self.problems.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(id, error.clone());
                    failed.push(error);
                }
            }
        }
        if voices.is_empty() && !failed.is_empty() {
            return Err(failed.join("; "));
        }
        Ok(voices)
    }
}

/// Each word's time from each character's: the characters between spaces make a word.
pub fn words_of_alignment(alignment: &Value) -> Vec<HeardWord> {
    let characters = alignment.get("characters").and_then(Value::as_array);
    let starts = alignment.get("character_start_times_seconds").and_then(Value::as_array);
    let ends = alignment.get("character_end_times_seconds").and_then(Value::as_array);
    let (Some(characters), Some(starts), Some(ends)) = (characters, starts, ends) else { return Vec::new() };
    let mut words = Vec::new();
    let mut current: Option<HeardWord> = None;
    for ((character, start), end) in characters.iter().zip(starts).zip(ends) {
        let (Some(character), Some(start), Some(end)) = (character.as_str(), start.as_f64(), end.as_f64()) else {
            continue;
        };
        if character.trim().is_empty() {
            words.extend(current.take());
            continue;
        }
        match current.as_mut() {
            Some(word) => {
                word.word.push_str(character);
                word.end = end;
            }
            None => current = Some(HeardWord { word: character.to_string(), start, end }),
        }
    }
    words.extend(current);
    words
}

/// The computer's voices through its voice server (speech/tts/own_speech.rs).
pub struct Computer {
    voice: Weak<crate::speech::voice_service::VoiceService>,
}

impl Computer {
    pub fn new(voice: &Arc<crate::speech::voice_service::VoiceService>) -> Computer {
        Computer { voice: Arc::downgrade(voice) }
    }

    fn speech(&self) -> Option<Arc<crate::speech::tts::own_speech::OwnSpeech>> {
        self.voice.upgrade().and_then(|voice| voice.own_speech().cloned())
    }
}

/// A computer's voice as an app sees it: its id, its licence, and whether a video with it may be published.
pub fn computer_card(model: &str, voice: &str) -> Value {
    let (license, publish) = match (model, voice) {
        ("piper", "denis" | "dmitri") => ("CC0", true),
        ("piper", "ruslan") => ("CC BY-NC-SA 4.0", false),
        ("silero", _) => ("CC BY-NC 4.0", false),
        _ => ("", false),
    };
    let license =
        if license.is_empty() { crate::tr!("лицензия не выяснена") } else { license.to_string() };
    json!({
        "id": format!("{model}/{voice}"),
        "title": voice,
        "provider": COMPUTER,
        "model": model,
        "voice": voice,
        "license": license,
        "publish": publish,
        "cloud": false,
        "paid": false,
    })
}

/// No voice server at all: an app goes on without a voice (the feed's silent draft knows these words).
fn no_server() -> String {
    crate::tr!("голосов компьютера нет: сервер голоса не настроен")
}

impl VoiceProvider for Computer {
    fn id(&self) -> &'static str {
        COMPUTER
    }

    fn card(&self) -> ProviderCard {
        let ready = self.speech().is_some();
        ProviderCard {
            id: COMPUTER,
            title: crate::tr!("Голоса компьютера"),
            ready,
            needs: (!ready).then(|| crate::tr!("сервер голоса не настроен")),
            cloud: false,
            paid: false,
            words: false,
            key: None,
            about: crate::tr!("Piper, Silero и свои голоса — на этом компьютере; текст не уходит из дома"),
            problem: None,
        }
    }

    fn voices(&self) -> BoxFuture<'static, Result<Vec<Value>, String>> {
        let speech = self.speech();
        Box::pin(async move {
            let speech = speech.ok_or_else(no_server)?;
            let voices =
                speech.computer_voices().await.map_err(|error| crate::tr!("сервер голоса: {error}", error = error))?;
            Ok(voices
                .iter()
                .flat_map(|(model, voices)| voices.iter().map(move |voice| computer_card(model, voice)))
                .collect())
        })
    }

    fn synth(&self, line: Line, cancel: CancellationToken) -> BoxFuture<'static, Result<Spoken, String>> {
        let speech = self.speech();
        Box::pin(async move {
            let speech = speech.ok_or_else(no_server)?;
            let (model, name) = line
                .voice
                .split_once('/')
                .ok_or_else(|| crate::tr!("голос компьютера — «движок/голос», например piper/denis"))?;
            let wav = speech
                .synthesize_wav_as(model, name, &line.text, &cancel)
                .await
                .map_err(|error| crate::tr!("голос не озвучил: {error}", error = error))?;
            Ok(Spoken { bytes: wav.to_vec(), ext: "wav", alignment: None, words: Vec::new() })
        })
    }

    /// Where its voice server is (TTS_OWN_URL, what the apps' lines are voiced by): at home, or a cloud's.
    fn reach(&self) -> Reach {
        match self.speech() {
            Some(speech) if !is_local_url(Some(speech.env_base_url())) => Reach::Foreign,
            _ => Reach::Home,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider of two voices, for the registry's routing.
    struct Fake(&'static str, bool);

    impl VoiceProvider for Fake {
        fn id(&self) -> &'static str {
            self.0
        }
        fn card(&self) -> ProviderCard {
            ProviderCard {
                id: self.0,
                title: "fake".into(),
                ready: self.1,
                needs: None,
                cloud: self.0 != COMPUTER,
                paid: false,
                words: false,
                key: None,
                about: String::new(),
                problem: None,
            }
        }
        fn voices(&self) -> BoxFuture<'static, Result<Vec<Value>, String>> {
            let id = self.0;
            Box::pin(async move {
                if id == "broken" {
                    return Err("сломан".into());
                }
                Ok(vec![json!({ "id": format!("{id}/one") })])
            })
        }
        fn synth(&self, _: Line, _: CancellationToken) -> BoxFuture<'static, Result<Spoken, String>> {
            Box::pin(async { Err("нет".into()) })
        }
    }

    /// The regulator lets every road out («Любые облака»).
    fn open() -> Gate {
        Arc::new(|_| true)
    }

    #[tokio::test]
    async fn a_voice_goes_to_its_provider_by_its_id_and_the_list_keeps_a_failure_on_the_card() {
        let voices = Voices::new(open());
        voices.register(Arc::new(Fake(COMPUTER, true)));
        voices.register(Arc::new(Fake("elevenlabs", true)));
        voices.register(Arc::new(Fake("broken", true)));
        voices.register(Arc::new(Fake("sleeping", false)));
        let (provider, own) = voices.route("elevenlabs/abc").unwrap();
        assert_eq!((provider.id(), own.as_str()), ("elevenlabs", "abc"));
        let (provider, own) = voices.route("elevenlabs:abc").unwrap();
        assert_eq!((provider.id(), own.as_str()), ("elevenlabs", "abc"), "the older id of the feed's settings");
        let (provider, own) = voices.route("piper/denis").unwrap();
        assert_eq!((provider.id(), own.as_str()), (COMPUTER, "piper/denis"), "an engine is the computer's");
        let all = voices.all().await.unwrap();
        let ids: Vec<&str> = all.iter().filter_map(|voice| voice["id"].as_str()).collect();
        assert_eq!(ids, ["computer/one", "elevenlabs/one"], "the one not ready is not asked");
        let cards = voices.cards();
        assert_eq!(cards.iter().find(|card| card.id == "broken").unwrap().problem.as_deref(), Some("сломан"));
        assert!(
            Voices::new(open()).all().await.unwrap_err().contains("сервер голоса не настроен"),
            "no provider: no voice"
        );
    }

    #[test]
    fn words_come_from_the_characters_between_spaces() {
        let alignment = json!({
            "characters": ["Н", "о", "ч", "ь", " ", "т", "и", "х", "а", "."],
            "character_start_times_seconds": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            "character_end_times_seconds": [0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1],
        });
        let words = words_of_alignment(&alignment);
        assert_eq!(words.len(), 2);
        assert_eq!((words[0].word.as_str(), words[0].start, words[0].end), ("Ночь", 0.1, 0.5));
        assert_eq!((words[1].word.as_str(), words[1].start, words[1].end), ("тиха.", 0.6, 1.1));
        assert!(words_of_alignment(&json!({})).is_empty());
    }
}
