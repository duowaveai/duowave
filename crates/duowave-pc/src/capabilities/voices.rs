//! The voices of the apps' lines — the computer's part, added to «Приложения»'s `capabilities::voices` under its name:
//! ElevenLabs by the owner's key, MP3 with each character's time (the station's build has none).

pub use duowave_apps::capabilities::voices::*;

/// ElevenLabs by the owner's key: MP3 with each character's time.
pub use eleven::{ElevenLabs, ELEVENLABS};

mod eleven {
    use std::sync::Arc;
    use std::time::Duration;

    use base64::Engine;
    use futures::future::BoxFuture;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use super::{words_of_alignment, Line, ProviderCard, Spoken, VoiceProvider};
    use crate::core::helpers::Reach;
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
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::core::helpers::Reach;

    fn open() -> Gate {
        Arc::new(|_| true)
    }

    /// A voice server at home: it voices every line (a short WAV) and counts them.
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
    fn computer_voices(own_url: &str, gate: Gate) -> (Voices, Arc<crate::speech::VoiceService>) {
        let config = crate::config::Config::for_tests(&crate::config::Env::from_pairs([("TTS_OWN_URL", own_url)]));
        let voice = Arc::new(crate::speech::VoiceService::new(&config.gateway, crate::core::events::EventBus::new()));
        let voices = Voices::new(gate);
        voices.register(Arc::new(Computer::new(&voice)));
        (voices, voice)
    }

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

    #[tokio::test]
    async fn elevenlabs_voices_by_the_key_in_the_safe_and_gives_the_sound_with_its_times() {
        use crate::speech::asr::openai_transcriber::testing::{Reply, ScriptedServer};
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
