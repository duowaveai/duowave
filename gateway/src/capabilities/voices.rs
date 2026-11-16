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

use crate::helpers::Reach;
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

/// ElevenLabs by the owner's key: MP3 with each character's time.
#[cfg(feature = "pc")]
pub use eleven::{ElevenLabs, ELEVENLABS};

#[cfg(feature = "pc")]
mod eleven {
    use std::sync::Arc;
    use std::time::Duration;

    use base64::Engine;
    use futures::future::BoxFuture;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use super::{words_of_alignment, Line, ProviderCard, Spoken, VoiceProvider};
    use crate::helpers::Reach;
    use crate::secrets::SecretsService;

    pub const ELEVENLABS: &str = "elevenlabs";
    /// The owner's key in the gateway's safe: typed in the panel («Голос» → «Облачные голоса»), never by an app.
    pub const KEY: &str = "voice:elevenlabs:key";
    const URL: &str = "https://api.elevenlabs.io";
    /// The model the series were voiced with (series-feed `tools/voice.ts`).
    const MODEL: &str = "eleven_v4";
    /// How long one line may take.
    const LINE_LIMIT: Duration = Duration::from_secs(120);

    pub struct ElevenLabs {
        secrets: Arc<SecretsService>,
        base: String,
    }

    impl ElevenLabs {
        pub fn new(secrets: &Arc<SecretsService>) -> ElevenLabs {
            ElevenLabs { secrets: secrets.clone(), base: URL.into() }
        }

        #[cfg(test)]
        pub fn at(secrets: &Arc<SecretsService>, base: &str) -> ElevenLabs {
            ElevenLabs { secrets: secrets.clone(), base: base.trim_end_matches('/').into() }
        }

        fn key(&self) -> Result<String, String> {
            self.secrets
                .get(KEY)
                .ok_or_else(|| crate::tr!("ключа ElevenLabs нет: он вводится в пульте, «Голос» → «Облачные голоса»"))
        }
    }

    /// One request to ElevenLabs: its answer as JSON, or its status and words.
    async fn ask(request: reqwest::RequestBuilder, key: &str) -> Result<Value, String> {
        let answer = request.header("xi-api-key", key).send().await.map_err(|error| format!("ElevenLabs: {error}"))?;
        let status = answer.status();
        let text = answer.text().await.map_err(|error| format!("ElevenLabs: {error}"))?;
        if !status.is_success() {
            let why: String = text.chars().take(300).collect();
            return Err(match status.as_u16() {
                401 => crate::tr!("ElevenLabs не принял ключ (401): {why}", why = why),
                402 | 429 => crate::tr!(
                    "ElevenLabs: знаки тарифа кончились или их не хватает ({status}): {why}",
                    status = status,
                    why = why
                ),
                _ => format!("ElevenLabs {status}: {why}"),
            });
        }
        serde_json::from_str(&text).map_err(|error| crate::tr!("ElevenLabs ответил не JSON: {error}", error = error))
    }

    /// The licence of the plan: the free one asks to name ElevenLabs and is not for commerce.
    fn licence(tier: Option<&str>) -> (String, bool) {
        match tier {
            Some("free") => (crate::tr!("ElevenLabs Free: только с подписью ElevenLabs, не для заработка"), false),
            Some(_) => (crate::tr!("ElevenLabs: коммерческая лицензия вашего тарифа"), true),
            None => (crate::tr!("ElevenLabs: по условиям вашего тарифа"), false),
        }
    }

    impl VoiceProvider for ElevenLabs {
        fn id(&self) -> &'static str {
            ELEVENLABS
        }

        fn card(&self) -> ProviderCard {
            let mask = self.secrets.mask(KEY);
            let ready = mask.set;
            ProviderCard {
                id: ELEVENLABS,
                title: "ElevenLabs".into(),
                ready,
                needs: (!ready).then(|| crate::tr!("ключ ElevenLabs — в пульте, «Голос» → «Облачные голоса»")),
                cloud: true,
                paid: true,
                words: true,
                key: Some(mask),
                about: crate::tr!(
                    "голоса ElevenLabs по вашему ключу: текст реплик уходит в ElevenLabs, тратятся знаки тарифа"
                ),
                problem: None,
            }
        }

        fn voices(&self) -> BoxFuture<'static, Result<Vec<Value>, String>> {
            let key = self.key();
            let base = self.base.clone();
            Box::pin(async move {
                let key = key?;
                let client = crate::utils::http::client();
                let listed =
                    ask(client.get(format!("{base}/v1/voices")).timeout(Duration::from_secs(15)), &key).await?;
                // The plan says whether a video with these voices may be published; a key without the right to read
                // it leaves that unknown.
                let tier =
                    ask(client.get(format!("{base}/v1/user/subscription")).timeout(Duration::from_secs(10)), &key)
                        .await
                        .ok()
                        .and_then(|plan| plan.get("tier").and_then(Value::as_str).map(str::to_string));
                let (license, publish) = licence(tier.as_deref());
                let voices = listed.get("voices").and_then(Value::as_array).cloned().unwrap_or_default();
                Ok(voices
                    .iter()
                    .filter_map(|voice| {
                        let id = voice.get("voice_id").and_then(Value::as_str)?;
                        let title = voice.get("name").and_then(Value::as_str).unwrap_or(id);
                        let mut card = json!({
                            "id": format!("{ELEVENLABS}/{id}"),
                            "title": title,
                            "provider": ELEVENLABS,
                            "license": license,
                            "publish": publish,
                            "cloud": true,
                            "paid": true,
                        });
                        if let Some(lang) = voice.pointer("/labels/language").and_then(Value::as_str) {
                            card["lang"] = json!(lang);
                        }
                        Some(card)
                    })
                    .collect())
            })
        }

        fn synth(&self, line: Line, cancel: CancellationToken) -> BoxFuture<'static, Result<Spoken, String>> {
            let key = self.key();
            let base = self.base.clone();
            Box::pin(async move {
                let key = key?;
                let voice = line.voice;
                if voice.is_empty() || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(crate::tr!("нет такого голоса ElevenLabs: {voice}", voice = voice));
                }
                let mut body = json!({ "text": line.text, "model_id": MODEL });
                let given = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
                if let Some(lang) = given(line.lang) {
                    body["language_code"] = json!(lang);
                }
                // The lines around it keep the intonation from line to line, as the series were voiced.
                if let Some(previous) = given(line.previous) {
                    body["previous_text"] = json!(previous);
                }
                if let Some(next) = given(line.next) {
                    body["next_text"] = json!(next);
                }
                let request = crate::utils::http::client()
                    .post(format!("{base}/v1/text-to-speech/{voice}/with-timestamps?output_format=mp3_44100_128"))
                    .timeout(LINE_LIMIT)
                    .json(&body);
                let answer = tokio::select! {
                    answer = ask(request, &key) => answer?,
                    _ = cancel.cancelled() => return Err(crate::tr!("озвучка остановлена")),
                };
                let audio = answer
                    .get("audio_base64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| crate::tr!("ElevenLabs не прислал звук"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(audio)
                    .map_err(|error| crate::tr!("ElevenLabs прислал испорченный звук: {error}", error = error))?;
                let alignment = answer.get("alignment").filter(|value| value.is_object()).cloned();
                let words = alignment.as_ref().map(words_of_alignment).unwrap_or_default();
                Ok(Spoken { bytes, ext: "mp3", alignment, words })
            })
        }

        fn balance(&self) -> BoxFuture<'static, Result<Option<Value>, String>> {
            let key = self.key();
            let base = self.base.clone();
            Box::pin(async move {
                let key = key?;
                let client = crate::utils::http::client();
                let plan =
                    ask(client.get(format!("{base}/v1/user/subscription")).timeout(Duration::from_secs(10)), &key)
                        .await?;
                Ok(Some(json!({
                    "used": plan.get("character_count"),
                    "limit": plan.get("character_limit"),
                    "tier": plan.get("tier"),
                    "unit": crate::tr!("знаков"),
                })))
            })
        }

        fn set_key(&self, key: Option<&str>) -> Result<(), String> {
            let key = key.map(str::trim).filter(|key| !key.is_empty());
            if key.is_some_and(|key| key.chars().any(char::is_whitespace) || key.len() > 200) {
                return Err(crate::tr!("это не похоже на ключ ElevenLabs"));
            }
            self.secrets.set(KEY, key).map_err(|error| error.to_string())
        }

        /// A cloud by the owner's key: the lines leave the house.
        fn reach(&self) -> Reach {
            Reach::Foreign
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

    /// A voice server at home: it voices every line (a short WAV) and counts them.
    #[cfg(feature = "pc")]
    async fn voice_server() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let lines = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = lines.clone();
        let router = axum::Router::new().route(
            "/v1/audio/speech",
            axum::routing::post(move || {
                let lines = counted.clone();
                async move {
                    lines.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    crate::speech::audio::pcm::encode_wav(&[0i16; 160], 16000)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        (url, lines)
    }

    /// The computer's voices with its voice server at this address, under this regulator; the voice service with them
    /// (the provider holds it weakly).
    #[cfg(feature = "pc")]
    fn computer_voices(own_url: &str, gate: Gate) -> (Voices, Arc<crate::speech::VoiceService>) {
        let config = crate::config::Config::for_tests(&crate::config::Env::from_pairs([("TTS_OWN_URL", own_url)]));
        let voice = Arc::new(crate::speech::VoiceService::new(&config.gateway, crate::core::events::EventBus::new()));
        let voices = Voices::new(gate);
        voices.register(Arc::new(Computer::new(&voice)));
        (voices, voice)
    }

    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn only_at_home_keeps_the_apps_cloud_voices_out() {
        let at_home: Gate = Arc::new(|reach| reach == Reach::Home);
        let closed = "Регулятор «Что может уходить из дома» не пускает облака по ключам.";
        let line = |voice: &str| Line { voice: voice.into(), text: "Да".into(), ..Line::default() };

        // ElevenLabs is a cloud by the owner's key: not ready, no voices, no line while only the home may be reached.
        let secrets =
            Arc::new(crate::secrets::SecretsService::new(Arc::new(crate::storage::db::Db::memory()), None).unwrap());
        let shut = Voices::new(at_home.clone());
        shut.register(Arc::new(ElevenLabs::at(&secrets, "https://api.elevenlabs.example.invalid")));
        assert!(shut.all().await.unwrap_err().contains("сервер голоса не настроен"), "no key: nothing would voice");
        shut.get(ELEVENLABS).unwrap().set_key(Some("sk-test-key")).unwrap();
        let eleven = shut.get(ELEVENLABS).unwrap();
        let card = eleven.card();
        assert_eq!((card.ready, card.needs.as_deref()), (false, Some(closed)));
        assert_eq!(eleven.voices().await.unwrap_err(), closed);
        assert_eq!(eleven.synth(line("v1"), CancellationToken::new()).await.err().as_deref(), Some(closed));
        assert_eq!(shut.all().await.unwrap_err(), closed, "an app goes on without a voice, told why");
        let open = Voices::new(open());
        open.register(Arc::new(ElevenLabs::at(&secrets, "https://api.elevenlabs.example.invalid")));
        assert!(open.get(ELEVENLABS).unwrap().card().ready, "«Любые облака»: its key is all it needs");

        // The computer's voice server at a cloud's address: the same.
        let (cloud, _voice) = computer_voices("https://tts.example.invalid/v1", at_home.clone());
        let computer = cloud.get(COMPUTER).unwrap();
        assert_eq!((computer.card().ready, computer.card().needs.as_deref()), (false, Some(closed)));
        assert_eq!(computer.synth(line("piper/denis"), CancellationToken::new()).await.err().as_deref(), Some(closed));
        assert_eq!(cloud.all().await.unwrap_err(), closed);

        // At home it voices under every position of the regulator.
        let (url, lines) = voice_server().await;
        let (home, _voice) = computer_voices(&url, at_home);
        let computer = home.get(COMPUTER).unwrap();
        assert!(computer.card().ready);
        let spoken = computer.synth(line("piper/denis"), CancellationToken::new()).await.unwrap();
        assert_eq!((spoken.ext, lines.load(std::sync::atomic::Ordering::SeqCst)), ("wav", 1));
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

    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn elevenlabs_voices_by_the_key_in_the_safe_and_gives_the_sound_with_its_times() {
        use crate::speech::asr::openai_transcriber::tests::{Reply, ScriptedServer};
        use base64::Engine;
        let secrets =
            Arc::new(crate::secrets::SecretsService::new(Arc::new(crate::storage::db::Db::memory()), None).unwrap());
        let server = ScriptedServer::start().await;
        let eleven = ElevenLabs::at(&secrets, &server.origin);
        assert!(!eleven.card().ready, "no key — not ready");
        assert!(eleven.voices().await.unwrap_err().contains("в пульте"));
        assert!(eleven.set_key(Some("not a key")).is_err());
        eleven.set_key(Some(" sk-test-key ")).unwrap();
        let card = eleven.card();
        assert!(card.ready && card.cloud && card.paid && card.words);
        assert!(!serde_json::to_string(&card).unwrap().contains("sk-test-key"), "the card never shows the key");

        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: json!({ "voices": [{ "voice_id": "v1", "name": "Rachel", "labels": { "language": "en" } }] })
                .to_string(),
        });
        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: json!({ "tier": "creator" }).to_string(),
        });
        let voices = eleven.voices().await.unwrap();
        assert_eq!(voices[0]["id"], "elevenlabs/v1");
        assert_eq!(voices[0]["publish"], true, "a paid plan may publish");
        assert_eq!(voices[0]["lang"], "en");

        let sound = base64::engine::general_purpose::STANDARD.encode(b"ID3 fake mp3");
        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: json!({
                "audio_base64": sound,
                "alignment": {
                    "characters": ["Д", "а"],
                    "character_start_times_seconds": [0.0, 0.1],
                    "character_end_times_seconds": [0.1, 0.3],
                },
            })
            .to_string(),
        });
        let line = |voice: &str| Line { voice: voice.into(), text: "Да".into(), ..Line::default() };
        let asked = Line { lang: Some("ru".into()), previous: Some("Кто там?".into()), ..line("v1") };
        let spoken = eleven.synth(asked, CancellationToken::new()).await.unwrap();
        assert_eq!((spoken.ext, spoken.bytes.as_slice()), ("mp3", b"ID3 fake mp3".as_slice()));
        assert_eq!(spoken.words.len(), 1);
        assert!(spoken.alignment.is_some());
        {
            let asked = server.requests.lock().unwrap();
            let line = asked.last().unwrap();
            assert!(line.path.starts_with("/v1/text-to-speech/v1/with-timestamps"));
            assert!(
                line.head.to_ascii_lowercase().contains("xi-api-key: sk-test-key"),
                "the key from the safe, trimmed"
            );
            let body: Value = serde_json::from_slice(&line.body).unwrap();
            assert_eq!((body["model_id"].as_str(), body["language_code"].as_str()), (Some("eleven_v4"), Some("ru")));
            assert_eq!((body["previous_text"].as_str(), body.get("next_text")), (Some("Кто там?"), None));
        }

        server.then(Reply::Answer {
            status: 401,
            content_type: "application/json",
            body: "{\"detail\":\"invalid\"}".into(),
        });
        let refused = eleven.synth(line("v1"), CancellationToken::new()).await.err().unwrap();
        assert!(refused.contains("не принял ключ"));
        assert!(eleven.synth(line("../x"), CancellationToken::new()).await.is_err(), "no path in a voice id");
    }
}
