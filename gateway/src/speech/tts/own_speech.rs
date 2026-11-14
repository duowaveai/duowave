use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use regex::Regex;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;

use super::say_latin::say_latin;
use super::say_numbers::say_numbers;
use crate::config::OwnSpeechConfig;
use crate::speech::asr::transcript::is_js_space;
use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
use crate::speech::audio::ogg_opus_encoder::{encode_ogg_opus, EncodeRate};
use crate::speech::audio::opus_pcm_decoder::{OpusPcmDecoder, SPEECH_SAMPLE_RATE};
use crate::speech::audio::pcm::{decode_wav, encode_wav, try_resample};
use crate::utils::http::client;

/// Our synthesis of one phrase: Ogg/Opus, whole (the server's answer is read to its end). The TS gave it as an
/// async iterable of one chunk (the shared Synthesis of the Yandex session).
#[derive(Debug, Clone, PartialEq)]
pub struct Synthesis {
    /// "audio/opus": Ogg/Opus.
    pub format: String,
    pub audio: Bytes,
}

/// Synthesis markup, the way Yandex writes it in output_speech.
static MARKUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<speaker[^>]*>|sil<\[[0-9]+\]>|<\[[^\]]*\]>").expect("a valid pattern"));

/// The text without synthesis markup: other engines would read it aloud. (The same as the station session's
/// withoutMarkup in gateway/uniproxy/spoken-answer.)
fn without_markup(text: &str) -> String {
    let spaced = MARKUP.replace_all(text, " ");
    spaced.split(is_js_space).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Phrases in flight and waiting: for tests and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteLoad {
    pub active: usize,
    pub waiting: usize,
}

/// Where a voice is synthesized and how: the env's server (Silero, Piper) or a cloned voice's own server. A cloned
/// voice's model speaks one phrase at a time, while an answer asks for all its phrases at once (spoken-answer): the
/// route queues them here, and a phrase's timeout starts when it is sent, not while it waits.
pub struct SpeechRoute {
    base_url: String,
    timeout_ms: u64,
    /// None: as many at once as asked.
    max_concurrent: Option<usize>,
    slots: Option<Semaphore>,
    load: Mutex<RouteLoad>,
    /// Told when the server does not answer at all (refused, gone): it can be started again.
    on_unreachable: Option<Box<dyn Fn() + Send + Sync>>,
}

/// A place on the route; it frees itself when dropped.
pub struct RoutePlace<'a> {
    route: &'a SpeechRoute,
    _permit: Option<SemaphorePermit<'a>>,
}

impl Drop for RoutePlace<'_> {
    fn drop(&mut self) {
        let mut load = self.route.lock();
        load.active = load.active.saturating_sub(1);
    }
}

/// A phrase waiting for its place.
struct Waiting<'a>(&'a SpeechRoute);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut load = self.0.lock();
        load.waiting = load.waiting.saturating_sub(1);
    }
}

impl SpeechRoute {
    /// A route that takes every phrase at once.
    pub fn new(base_url: &str, timeout_ms: u64) -> SpeechRoute {
        SpeechRoute::build(base_url, timeout_ms, None, None)
    }

    /// A route that takes `max_concurrent` phrases at a time, the rest wait their turn in order.
    pub fn limited(
        base_url: &str,
        timeout_ms: u64,
        max_concurrent: usize,
        on_unreachable: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> SpeechRoute {
        SpeechRoute::build(base_url, timeout_ms, Some(max_concurrent), on_unreachable)
    }

    fn build(
        base_url: &str,
        timeout_ms: u64,
        max_concurrent: Option<usize>,
        on_unreachable: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> SpeechRoute {
        SpeechRoute {
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout_ms,
            max_concurrent,
            slots: max_concurrent.map(Semaphore::new),
            load: Mutex::new(RouteLoad::default()),
            on_unreachable,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Phrases sent at a time; None: as many as asked.
    pub fn max_concurrent(&self) -> Option<usize> {
        self.max_concurrent
    }

    pub fn load(&self) -> RouteLoad {
        *self.lock()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RouteLoad> {
        self.load.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A place in the queue; a cancel while waiting leaves it.
    pub async fn acquire(&self, cancel: &CancellationToken) -> Result<RoutePlace<'_>> {
        let permit = match &self.slots {
            None => None,
            Some(slots) => match slots.try_acquire() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    if cancel.is_cancelled() {
                        bail!("aborted while waiting for the voice");
                    }
                    // Counted while waiting, also when the caller drops the wait.
                    self.lock().waiting += 1;
                    let waiting = Waiting(self);
                    let waited = tokio::select! {
                        permit = slots.acquire() => permit.ok(),
                        _ = cancel.cancelled() => None,
                    };
                    drop(waiting);
                    match waited {
                        Some(permit) => Some(permit),
                        None => bail!("aborted while waiting for the voice"),
                    }
                }
            },
        };
        self.lock().active += 1;
        Ok(RoutePlace { route: self, _permit: permit })
    }

    /// The server did not answer at all: whoever runs it is told (a cloned voice's server is started again).
    pub fn unreachable(&self) {
        if let Some(told) = &self.on_unreachable {
            told();
        }
    }
}

impl std::fmt::Debug for SpeechRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechRoute")
            .field("base_url", &self.base_url)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_concurrent", &self.max_concurrent)
            .field("load", &self.load())
            .finish()
    }
}

/// Whether a synthesis failed because nothing answered at all: the voice server is not up (yet).
pub fn is_unreachable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.downcast_ref::<reqwest::Error>().is_some_and(unreachable))
}

/// A server's Ogg/Opus as a WAV: 16 kHz mono, the rate our decoder gives.
fn opus_to_wav(audio: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = OpusPcmDecoder::new();
    let mut pcm = Vec::new();
    for event in OggOpusDemuxer::new().push(audio) {
        match event {
            OggOpusEvent::Head { head, .. } => decoder.reset(&head),
            OggOpusEvent::Packet { data, .. } => pcm.extend(decoder.decode(&data)),
        }
    }
    if pcm.is_empty() {
        bail!("own speech: no sound in the server's Ogg/Opus");
    }
    Ok(encode_wav(&pcm, SPEECH_SAMPLE_RATE))
}

/// Nothing answered: refused, reset, unknown host — not an HTTP error from a server that is there, not a timeout.
fn unreachable(error: &reqwest::Error) -> bool {
    if error.is_timeout() {
        return false;
    }
    if error.is_connect() {
        return true;
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            if matches!(io.kind(), ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | BrokenPipe)
            {
                return true;
            }
        }
        // hyper: the other side closed the connection before its answer (undici's UND_ERR_SOCKET).
        if current.to_string().contains("connection closed before message completed") {
            return true;
        }
        cause = current.source();
    }
    false
}

struct Chosen {
    config: OwnSpeechConfig,
    route: Arc<SpeechRoute>,
}

/// Our own synthesis through an OpenAI-compatible `POST /audio/speech`. The station plays Ogg/Opus from TtsSpeak, so
/// a WAV comes back encoded. Yandex's markup is taken out: other engines would read it aloud.
pub struct OwnSpeech {
    env_route: Arc<SpeechRoute>,
    chosen: Mutex<Chosen>,
}

impl OwnSpeech {
    pub fn new(config: OwnSpeechConfig) -> OwnSpeech {
        let env_route = Arc::new(SpeechRoute::new(&config.base_url, config.timeout_ms));
        OwnSpeech { chosen: Mutex::new(Chosen { config, route: env_route.clone() }), env_route }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Chosen> {
        self.chosen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What speaks now — the pair can change at runtime (the panel's model settings).
    pub fn description(&self) -> String {
        let chosen = self.lock();
        format!("{}/{} via {}/audio/speech", chosen.config.model, chosen.config.voice, chosen.route.base_url())
    }

    /// The server the current voice is asked from.
    pub fn base_url(&self) -> String {
        self.lock().route.base_url().to_string()
    }

    /// Another voice from the next request on: the server picks the engine by the pair in each call. Without a route
    /// the env's server speaks (Silero, Piper); a cloned voice brings its own.
    pub fn choose(&self, model: &str, voice: &str, route: Option<Arc<SpeechRoute>>) {
        let mut chosen = self.lock();
        chosen.config.model = model.to_string();
        chosen.config.voice = voice.to_string();
        chosen.route = route.unwrap_or_else(|| self.env_route.clone());
    }

    pub async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<Synthesis> {
        let (format, bytes) = self.ask(text, cancel).await?;
        if format == "opus" {
            return Ok(Synthesis { format: "audio/opus".into(), audio: bytes });
        }
        // Full band: at 16 kHz our voice sounded like a phone line next to Alice's. Encoding is CPU work, off the
        // async threads. The rate is the server's word: bounded, so a bogus one fails this phrase, not the gateway.
        let audio = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let wav = decode_wav(&bytes)?;
            encode_ogg_opus(&try_resample(&wav.pcm, wav.sample_rate, 48000)?, EncodeRate::FullBand)
        })
        .await
        .map_err(|error| anyhow!("own speech: the encoder stopped: {error}"))??;
        Ok(Synthesis { format: "audio/opus".into(), audio: Bytes::from(audio) })
    }

    /// The same phrase for a computer's speakers (the talk without a station, src/talk): a WAV, 16-bit mono. The
    /// server's WAV keeps its rate — only its header is written anew, whole; a server that answers in Ogg/Opus is
    /// decoded, which gives 16 kHz.
    pub async fn synthesize_wav(&self, text: &str, cancel: &CancellationToken) -> Result<Bytes> {
        let (format, bytes) = self.ask(text, cancel).await?;
        let wav = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            if format == "opus" {
                return opus_to_wav(&bytes);
            }
            let wav = decode_wav(&bytes)?;
            Ok(encode_wav(&wav.pcm, wav.sample_rate))
        })
        .await
        .map_err(|error| anyhow!("own speech: the decoder stopped: {error}"))??;
        Ok(Bytes::from(wav))
    }

    /// One phrase asked of the voice server: the format it answered in (`opus`, or a WAV) and its bytes.
    async fn ask(&self, text: &str, cancel: &CancellationToken) -> Result<(String, Bytes)> {
        let (config, route) = {
            let chosen = self.lock();
            (chosen.config.clone(), chosen.route.clone())
        };
        // The env's server keeps its key and format; a cloned voice's server is ours: no key, WAV.
        let env = Arc::ptr_eq(&route, &self.env_route);
        let api_key = if env { config.api_key.clone() } else { None };
        let format = if env { config.format.clone() } else { "wav".to_string() };
        let body = serde_json::json!({
            "model": config.model,
            "voice": config.voice,
            // Silero skips digits and Latin: numbers go in words, Latin in Russian letters.
            "input": say_numbers(&say_latin(&without_markup(text))),
            "response_format": format,
        });
        let place = route.acquire(cancel).await?;
        let mut request = client()
            .post(format!("{}/audio/speech", route.base_url()))
            .json(&body)
            .timeout(Duration::from_millis(route.timeout_ms()));
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        let exchange = async {
            let response = request.send().await?;
            let status = response.status();
            Ok::<_, reqwest::Error>((status, response.bytes().await?))
        };
        let answered = tokio::select! {
            answered = exchange => answered,
            _ = cancel.cancelled() => {
                drop(place);
                bail!("own speech cancelled");
            }
        };
        drop(place);
        let (status, bytes) = answered.inspect_err(|error| {
            if unreachable(error) {
                route.unreachable();
            }
        })?;
        if !status.is_success() {
            let text: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            bail!("own speech: HTTP {} {text}", status.as_u16());
        }
        Ok((format, bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::Router;
    use serde_json::Value;
    use tokio::net::TcpListener;

    use super::*;
    use crate::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
    use crate::speech::audio::pcm::{encode_wav, resample};
    use crate::speech::test_audio::voice;

    #[derive(Default)]
    struct Heard {
        requests: Mutex<Vec<(Value, Option<String>)>>,
        in_flight: AtomicUsize,
        most_at_once: AtomicUsize,
    }

    /// A voice server: answers every phrase with `ms` of speech at `rate` after `delay_ms`, as Piper (22 kHz) or a
    /// cloned voice (24 kHz, one phrase at a time) would.
    async fn voice_server(ms: f64, rate: u32, delay_ms: u64) -> (String, Arc<Heard>) {
        wav_server(encode_wav(&resample(&voice(ms, -20.0), 16000, rate), rate), delay_ms).await
    }

    /// A voice server that answers every phrase with this WAV after `delay_ms`.
    async fn wav_server(wav: Vec<u8>, delay_ms: u64) -> (String, Arc<Heard>) {
        let heard = Arc::new(Heard::default());
        let app = Router::new()
            .route(
                "/v1/audio/speech",
                post(move |State(heard): State<Arc<Heard>>, headers: HeaderMap, body: String| {
                    let wav = wav.clone();
                    async move {
                        let now = heard.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        heard.most_at_once.fetch_max(now, Ordering::SeqCst);
                        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
                        heard.requests.lock().unwrap().push((serde_json::from_str(&body).unwrap(), auth));
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        heard.in_flight.fetch_sub(1, Ordering::SeqCst);
                        ([("content-type", "audio/wav")], wav)
                    }
                }),
            )
            .with_state(heard.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, heard)
    }

    fn config(url: &str, format: &str) -> OwnSpeechConfig {
        OwnSpeechConfig {
            base_url: url.to_string(),
            api_key: None,
            model: "silero".into(),
            voice: "xenia".into(),
            format: format.into(),
            timeout_ms: 20_000,
        }
    }

    fn packets(audio: &[u8]) -> usize {
        OggOpusDemuxer::new().push(audio).iter().filter(|event| matches!(event, OggOpusEvent::Packet { .. })).count()
    }

    #[tokio::test]
    async fn asks_an_openai_compatible_server_and_gives_the_station_ogg_opus_yandex_markup_taken_out() {
        // 0.5 s at 22050 Hz, as Piper voices speak.
        let (url, heard) = voice_server(500.0, 22050, 0).await;
        let speech = OwnSpeech::new(config(&url, "wav"));
        let synthesis =
            speech.synthesize("<speaker is_whisper=\"true\"> Привет.", &CancellationToken::new()).await.unwrap();
        assert_eq!(
            heard.requests.lock().unwrap().last().unwrap().0,
            serde_json::json!({"model": "silero", "voice": "xenia", "input": "Привет.", "response_format": "wav"})
        );
        assert_eq!(synthesis.format, "audio/opus");
        assert_eq!(packets(&synthesis.audio), 25);
    }

    #[tokio::test]
    async fn gives_a_computer_the_phrase_as_a_wav_at_the_servers_rate_and_decodes_a_server_that_answers_in_opus() {
        // 0.5 s at 22050 Hz, as Piper voices speak: the WAV keeps the rate, its header is written whole.
        let (url, heard) = voice_server(500.0, 22050, 0).await;
        let speech = OwnSpeech::new(config(&url, "wav"));
        let wav = speech.synthesize_wav("<speaker voice=\"x\"> 5 минут", &CancellationToken::new()).await.unwrap();
        assert_eq!(heard.requests.lock().unwrap().last().unwrap().0["input"], "пять минут", "the same preparation");
        assert_eq!((&wav[0..4], &wav[8..12]), (&b"RIFF"[..], &b"WAVE"[..]));
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16, "16-bit");
        let decoded = decode_wav(&wav).unwrap();
        assert_eq!((decoded.sample_rate, decoded.pcm.len()), (22050, 11025));
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize, decoded.pcm.len() * 2);

        // A server in the opus format: decoded to PCM, which is 16 kHz.
        let opus = encode_ogg_opus(&resample(&voice(500.0, -20.0), 16000, 48000), EncodeRate::FullBand).unwrap();
        let (opus_url, _) = wav_server(opus, 0).await;
        let speech = OwnSpeech::new(config(&opus_url, "opus"));
        let wav = speech.synthesize_wav("Привет.", &CancellationToken::new()).await.unwrap();
        let decoded = decode_wav(&wav).unwrap();
        assert_eq!(decoded.sample_rate, 16000);
        assert!((7000..=8200).contains(&decoded.pcm.len()), "about half a second: {}", decoded.pcm.len());

        // Nothing listens there: the error says the server did not answer at all.
        let gone = OwnSpeech::new(config("http://127.0.0.1:9/v1", "wav"));
        let error = gone.synthesize_wav("Привет.", &CancellationToken::new()).await.unwrap_err();
        assert!(is_unreachable(&error), "{error:#}");
    }

    #[tokio::test]
    async fn sends_an_answers_phrases_one_at_a_time_each_timed_from_its_sending_in_wav_and_without_the_envs_key() {
        let (url, _) = voice_server(500.0, 22050, 0).await;
        // One phrase at a time takes 150 ms, as a cloned voice's model works through its lock.
        let (slow_url, slow) = voice_server(300.0, 24000, 150).await;
        let speech = OwnSpeech::new(OwnSpeechConfig {
            api_key: Some("env-key".into()),
            model: "tts-1".into(),
            voice: "alloy".into(),
            ..config(&url, "opus")
        });
        // Three phrases of 150 ms each under a 250 ms timeout: queued, none of them times out.
        let route = Arc::new(SpeechRoute::limited(&format!("{slow_url}/"), 250, 1, None));
        speech.choose("omnivoice", "v-abc", Some(route));
        assert_eq!(speech.description(), format!("omnivoice/v-abc via {slow_url}/audio/speech"));
        let cancel = CancellationToken::new();
        let phrases =
            futures::future::join_all(["Раз.", "Два.", "Три."].map(|text| speech.synthesize(text, &cancel))).await;
        let formats: Vec<String> = phrases.into_iter().map(|phrase| phrase.unwrap().format).collect();
        assert_eq!(formats, ["audio/opus", "audio/opus", "audio/opus"]);
        assert_eq!(slow.most_at_once.load(Ordering::SeqCst), 1);
        let seen = slow.requests.lock().unwrap();
        let inputs: Vec<&str> = seen.iter().map(|(body, _)| body["input"].as_str().unwrap()).collect();
        assert_eq!(inputs, ["Раз.", "Два.", "Три."]);
        assert_eq!(
            seen[0],
            (
                serde_json::json!({"model": "omnivoice", "voice": "v-abc", "input": "Раз.", "response_format": "wav"}),
                None
            )
        );

        // Back to the env's voice: its server, its format and key again.
        speech.choose("silero", "xenia", None);
        assert_eq!(speech.base_url(), url);
    }

    #[tokio::test]
    async fn lets_a_waiting_phrase_go_when_the_answer_is_cancelled_and_the_queue_moves_on() {
        let (url, _) = voice_server(500.0, 22050, 0).await;
        let (slow_url, _) = voice_server(300.0, 24000, 150).await;
        let route = Arc::new(SpeechRoute::limited(&slow_url, 5_000, 1, None));
        let speech = Arc::new(OwnSpeech::new(OwnSpeechConfig { timeout_ms: 5_000, ..config(&url, "wav") }));
        speech.choose("omnivoice", "v-abc", Some(route.clone()));
        let first = tokio::spawn({
            let speech = speech.clone();
            async move { speech.synthesize("Первая.", &CancellationToken::new()).await }
        });
        while route.load().active == 0 {
            tokio::task::yield_now().await;
        }
        let cancel = CancellationToken::new();
        let second = tokio::spawn({
            let (speech, cancel) = (speech.clone(), cancel.clone());
            async move { speech.synthesize("Вторая.", &cancel).await }
        });
        while route.load().waiting == 0 {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        let error = second.await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "aborted while waiting for the voice");
        first.await.unwrap().unwrap();
        assert_eq!(route.load(), RouteLoad { active: 0, waiting: 0 });
    }

    #[tokio::test]
    async fn tells_the_route_when_nothing_answers_at_all_so_the_server_can_be_started_again() {
        let (url, _) = voice_server(500.0, 22050, 0).await;
        let gone = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = gone.local_addr().unwrap().port();
        drop(gone);
        let told = Arc::new(AtomicUsize::new(0));
        let counter = told.clone();
        let speech = OwnSpeech::new(OwnSpeechConfig { timeout_ms: 5_000, ..config(&url, "wav") });
        let route = SpeechRoute::limited(
            &format!("http://127.0.0.1:{port}/v1"),
            5_000,
            1,
            Some(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
        );
        speech.choose("omnivoice", "v-abc", Some(Arc::new(route)));
        assert!(speech.synthesize("Привет.", &CancellationToken::new()).await.is_err());
        assert_eq!(told.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fails_the_phrase_not_the_gateway_when_the_servers_wav_names_a_bogus_rate() {
        let speech = |url: &str| OwnSpeech::new(config(url, "wav"));
        // The header says 1 Hz: at 48 kHz these 20 samples would make 20 s, a phrase's worth of them gigabytes.
        let mut bogus = encode_wav(&[100; 20], 22050);
        bogus[24..28].copy_from_slice(&1u32.to_le_bytes());
        let (url, _) = wav_server(bogus, 0).await;
        let error = speech(&url).synthesize("Привет.", &CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.to_string(), "WAV with 1 channel(s) at 1 Hz");

        // A rate audio has, but six minutes of it: more than resampling makes for a phrase.
        let (url, _) = wav_server(encode_wav(&vec![0; 360_000], 1_000), 0).await;
        let error = speech(&url).synthesize("Привет.", &CancellationToken::new()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "resampling 360000 samples from 1000 Hz to 48000 Hz makes 17280000: more than 14400000"
        );
    }

    #[test]
    fn takes_the_markup_out() {
        assert_eq!(without_markup("<speaker voice=\"x\">Раз sil<[300]> два <[a b]>  три"), "Раз два три");
    }
}
