//! Cloud hearing and voice set up in the panel (docs/plans/speaker-alone.md, stage 1): the station's gateway hears
//! and speaks through an OpenAI-compatible cloud — without the computer and without Yandex.
//!
//! - The choice lives in the setting `cloud_speech` (each gateway its own, placement/transfer.rs): for the hearing
//!   OpenAI, Groq or an address of one's own; for the voice OpenAI or an address of one's own; or off.
//! - The keys live in the safe. OpenAI's and Groq's are the brain's (`llm:openai:key`, `llm:groq:key`): one key for
//!   the brain, the hearing and the voice (the owner's decision 1, 16.11). An address of one's own keeps its own.
//! - Nothing set in the panel: the .env's cloud as before (a non-local ASR_BASE_URL or TTS_OWN_URL).
//! - Read on every use: a change in the panel or a new key speaks from the next request, without a restart. The
//!   recognizer and the voice are built again only when what they are built from changed.
//!
//! Only the station's gateway uses it: a computer hears and speaks with its own models.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::App;
use crate::config::{AsrConfig, OwnSpeechConfig};
use crate::core::identity::permissions::{CONNECTORS_VIEW, MODELS_MANAGE};
use crate::operations::networks::is_local_url;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::secrets::SecretsService;
use crate::settings::definitions::CLOUD_SPEECH;
use crate::settings::SettingsService;
use crate::speech::asr::speech_recognizer::{create_recognizer, SpeechRecognizer};
use crate::speech::tts::own_speech::OwnSpeech;

pub const OPENAI_URL: &str = "https://api.openai.com/v1";
pub const GROQ_URL: &str = "https://api.groq.com/openai/v1";
/// OpenAI's recognition: cheap, and it hears Russian well.
pub const OPENAI_HEARING_MODEL: &str = "gpt-4o-mini-transcribe";
pub const GROQ_HEARING_MODEL: &str = "whisper-large-v3-turbo";
pub const OPENAI_VOICE_MODEL: &str = "gpt-4o-mini-tts";
pub const OPENAI_VOICE: &str = "coral";
/// OpenAI's voices for `gpt-4o-mini-tts`, for the panel's list.
pub const OPENAI_VOICES: &[&str] =
    &["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse", "marin", "cedar"];
/// A phrase from the cloud takes longer than one from home.
const VOICE_TIMEOUT_MS: u64 = 15_000;
const CHECK_FOR: Duration = Duration::from_secs(8);

/// Who serves a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Openai,
    Groq,
    Custom,
    Off,
}

impl Service {
    pub fn title(self) -> &'static str {
        match self {
            Service::Openai => "OpenAI",
            Service::Groq => "Groq",
            Service::Custom => "Свой адрес",
            Service::Off => "Выключено",
        }
    }
}

/// The two parts: what is heard and what is said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Hearing,
    Voice,
}

impl Part {
    fn as_str(self) -> &'static str {
        match self {
            Part::Hearing => "hearing",
            Part::Voice => "voice",
        }
    }
}

/// One part as the panel set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub service: Service,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The setting `cloud_speech`: a part left out goes by the .env.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hearing: Option<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<Choice>,
}

impl Stored {
    pub fn read(settings: &SettingsService) -> Stored {
        settings.get(&CLOUD_SPEECH, None).and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
    }

    fn part(&self, part: Part) -> Option<&Choice> {
        match part {
            Part::Hearing => self.hearing.as_ref(),
            Part::Voice => self.voice.as_ref(),
        }
    }
}

/// Where a service's key lives in the safe. OpenAI's and Groq's are the brain's.
pub fn key_id(service: Service, part: Part) -> Option<String> {
    match service {
        Service::Openai => Some("llm:openai:key".into()),
        Service::Groq => Some("llm:groq:key".into()),
        Service::Custom => Some(format!("speech:cloud:{}:key", part.as_str())),
        Service::Off => None,
    }
}

fn base_of(choice: &Choice) -> Option<String> {
    match choice.service {
        Service::Openai => Some(OPENAI_URL.into()),
        Service::Groq => Some(GROQ_URL.into()),
        Service::Custom => choice
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/').to_string()),
        Service::Off => None,
    }
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

/// The cloud's hosts the panel set up, for «Выход в интернет» (src/egress): `(id, title, url)`.
pub fn panel_hosts(settings: &SettingsService) -> Vec<(String, String, String)> {
    let stored = Stored::read(settings);
    let mut hosts = Vec::new();
    if let Some(url) = stored.hearing.as_ref().and_then(base_of) {
        hosts.push(("hearing:cloud".into(), crate::tr!("Слух: облачное распознавание"), url));
    }
    if let Some(url) = stored.voice.as_ref().and_then(base_of) {
        hosts.push(("voice:cloud".into(), crate::tr!("Голос: облачный синтез"), url));
    }
    hosts
}

pub struct CloudSpeech {
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    /// The .env's recognition and voice: the way when the panel set nothing, and what a cloud's config starts from.
    env_asr: AsrConfig,
    env_voice: Option<OwnSpeechConfig>,
    /// Only the station's gateway hears and speaks through it.
    active: bool,
    hearing: Mutex<Option<(AsrKey, Arc<dyn SpeechRecognizer>)>>,
    voice: Mutex<Option<(VoiceKey, Arc<OwnSpeech>)>>,
}

/// What a recognizer was built from: a change builds it again.
type AsrKey = (String, Option<String>, String);
type VoiceKey = (String, Option<String>, String, String);

impl CloudSpeech {
    pub fn new(
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        env_asr: AsrConfig,
        env_voice: Option<OwnSpeechConfig>,
        active: bool,
    ) -> CloudSpeech {
        CloudSpeech {
            settings,
            secrets,
            env_asr,
            env_voice,
            active,
            hearing: Mutex::new(None),
            voice: Mutex::new(None),
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    fn stored(&self) -> Stored {
        Stored::read(&self.settings)
    }

    fn key(&self, service: Service, part: Part) -> Option<String> {
        key_id(service, part).and_then(|id| self.secrets.get(&id)).filter(|key| !key.trim().is_empty())
    }

    /// The recognition in the cloud as it stands: the panel's, else the .env's when it is not at home. None: none.
    pub fn hearing_config(&self) -> Option<AsrConfig> {
        let Some(choice) = self.stored().hearing else {
            let env = &self.env_asr;
            return (env.enabled && env.local.is_none() && !is_local_url(Some(&env.base_url))).then(|| env.clone());
        };
        let base_url = base_of(&choice)?;
        let api_key = self.key(choice.service, Part::Hearing);
        if choice.service != Service::Custom && api_key.is_none() {
            return None;
        }
        let model = non_empty(&choice.model).unwrap_or_else(|| match choice.service {
            Service::Groq => GROQ_HEARING_MODEL.into(),
            Service::Custom => self.env_asr.model.clone(),
            _ => OPENAI_HEARING_MODEL.into(),
        });
        Some(AsrConfig { enabled: true, base_url, api_key, model, local: None, ..self.env_asr.clone() })
    }

    /// The voice in the cloud as it stands: the panel's, else the .env's when it is not at home. None: none.
    pub fn voice_config(&self) -> Option<OwnSpeechConfig> {
        let Some(choice) = self.stored().voice else {
            return self.env_voice.clone().filter(|own| !is_local_url(Some(&own.base_url)));
        };
        let base_url = base_of(&choice)?;
        if choice.service == Service::Groq {
            return None;
        }
        let api_key = self.key(choice.service, Part::Voice);
        if choice.service != Service::Custom && api_key.is_none() {
            return None;
        }
        let env = self.env_voice.as_ref();
        let model = non_empty(&choice.model).unwrap_or_else(|| match choice.service {
            Service::Custom => env.map(|own| own.model.clone()).unwrap_or_else(|| OPENAI_VOICE_MODEL.into()),
            _ => OPENAI_VOICE_MODEL.into(),
        });
        let voice = non_empty(&choice.voice).unwrap_or_else(|| match choice.service {
            Service::Custom => env.map(|own| own.voice.clone()).unwrap_or_else(|| OPENAI_VOICE.into()),
            _ => OPENAI_VOICE.into(),
        });
        // Opus goes to the station as it came: its weak processor encodes nothing.
        Some(OwnSpeechConfig { base_url, api_key, model, voice, format: "opus".into(), timeout_ms: VOICE_TIMEOUT_MS })
    }

    /// The recognizer in the cloud now; built again when its settings or key changed.
    pub fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        if !self.active {
            return None;
        }
        let config = self.hearing_config();
        let mut kept = self.hearing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(config) = config else {
            *kept = None;
            return None;
        };
        let key: AsrKey = (config.base_url.clone(), config.api_key.clone(), config.model.clone());
        if let Some((built, recognizer)) = kept.as_ref() {
            if *built == key {
                return Some(recognizer.clone());
            }
        }
        let recognizer = create_recognizer(&config)?;
        tracing::info!(target: "speech", "cloud hearing: {}", recognizer.description());
        *kept = Some((key, recognizer.clone()));
        Some(recognizer)
    }

    /// Our voice in the cloud now; built again when its settings or key changed.
    pub fn speech(&self) -> Option<Arc<OwnSpeech>> {
        if !self.active {
            return None;
        }
        let config = self.voice_config();
        let mut kept = self.voice.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(config) = config else {
            *kept = None;
            return None;
        };
        let key: VoiceKey =
            (config.base_url.clone(), config.api_key.clone(), config.model.clone(), config.voice.clone());
        if let Some((built, speech)) = kept.as_ref() {
            if *built == key {
                return Some(speech.clone());
            }
        }
        let speech = Arc::new(OwnSpeech::new(config));
        tracing::info!(target: "speech", "cloud voice: {}", speech.description());
        *kept = Some((key, speech.clone()));
        Some(speech)
    }

    pub fn hears(&self) -> bool {
        self.active && self.hearing_config().is_some()
    }

    pub fn speaks(&self) -> bool {
        self.active && self.voice_config().is_some()
    }

    fn part_view(&self, part: Part) -> PartView {
        let stored = self.stored();
        let choice = stored.part(part).cloned();
        let (service, model, voice, url, from_env) = match &choice {
            Some(choice) => {
                (choice.service, non_empty(&choice.model), non_empty(&choice.voice), non_empty(&choice.url), false)
            }
            None => {
                let env = match part {
                    Part::Hearing => self.hearing_config().map(|asr| (asr.base_url, asr.model, None)),
                    Part::Voice => self.voice_config().map(|own| (own.base_url, own.model, Some(own.voice))),
                };
                match env {
                    Some((url, model, voice)) => (Service::Custom, Some(model), voice, Some(url), true),
                    None => (Service::Off, None, None, None, false),
                }
            }
        };
        let key_set = !from_env && self.key(service, part).is_some();
        let ready = match part {
            Part::Hearing => self.hearing_config().is_some(),
            Part::Voice => self.voice_config().is_some(),
        };
        PartView {
            service,
            model,
            voice,
            url,
            ready,
            key_set,
            key_shared: matches!(service, Service::Openai | Service::Groq),
            from_env,
        }
    }

    pub fn view(&self) -> CloudSpeechView {
        CloudSpeechView {
            active: self.active,
            hearing: self.part_view(Part::Hearing),
            voice: self.part_view(Part::Voice),
            defaults: Defaults {
                openai_hearing_model: OPENAI_HEARING_MODEL,
                groq_hearing_model: GROQ_HEARING_MODEL,
                openai_voice_model: OPENAI_VOICE_MODEL,
                openai_voice: OPENAI_VOICE,
                openai_voices: OPENAI_VOICES,
            },
        }
    }

    /// The panel's change of one part; a key given goes to the safe where that service keeps it.
    pub fn set(&self, change: &Change, actor: Option<crate::core::events::Actor>) -> Result<(), ApiError> {
        if change.part == Part::Voice && change.service == Service::Groq {
            return Err(ApiError::bad_request(crate::tr!(
                "У Groq нет подходящего голоса: выберите OpenAI или свой адрес."
            )));
        }
        if change.service == Service::Custom {
            let url = non_empty(&change.url).ok_or_else(|| {
                ApiError::bad_request(crate::tr!("Нужен адрес сервиса, например https://example.com/v1"))
            })?;
            if url::Url::parse(&url).is_err() {
                return Err(ApiError::bad_request(crate::tr!("Адрес не читается: {url}", url = url)));
            }
        }
        if let (Some(key), Some(id)) = (non_empty(&change.key), key_id(change.service, change.part)) {
            self.secrets.set(&id, Some(&key))?;
        }
        let mut stored = self.stored();
        let choice = Choice {
            service: change.service,
            model: non_empty(&change.model),
            voice: non_empty(&change.voice),
            url: if change.service == Service::Custom { non_empty(&change.url) } else { None },
        };
        match change.part {
            Part::Hearing => stored.hearing = Some(choice),
            Part::Voice => stored.voice = Some(choice),
        }
        let value = serde_json::to_value(&stored).map_err(ApiError::internal)?;
        self.settings.set(&CLOUD_SPEECH, &value, actor)
    }

    /// Whether the service answers with this key: `GET /models`. None — it does; Some — why not, in words.
    pub async fn check(&self, part: Part) -> Option<String> {
        let (base, key) = match part {
            Part::Hearing => match self.hearing_config() {
                Some(asr) => (asr.base_url, asr.api_key),
                None => return Some(self.why_not(part)),
            },
            Part::Voice => match self.voice_config() {
                Some(own) => (own.base_url, own.api_key),
                None => return Some(self.why_not(part)),
            },
        };
        check_models(&base, key.as_deref()).await
    }

    fn why_not(&self, part: Part) -> String {
        match self.stored().part(part) {
            None | Some(Choice { service: Service::Off, .. }) => crate::tr!("Не настроено."),
            Some(choice) if choice.service != Service::Custom => {
                crate::tr!(
                    "Нет ключа {service}: вставьте его здесь или в «Мышлении».",
                    service = choice.service.title()
                )
            }
            Some(_) => crate::tr!("Нет адреса сервиса."),
        }
    }
}

/// `GET <base>/models` with the key: whether a cloud answers it. None — it does.
pub async fn check_models(base: &str, key: Option<&str>) -> Option<String> {
    let mut request =
        crate::utils::http::client().get(format!("{}/models", base.trim_end_matches('/'))).timeout(CHECK_FOR);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(answer) if answer.status().is_success() => None,
        Ok(answer) if answer.status().as_u16() == 401 || answer.status().as_u16() == 403 => {
            Some(crate::tr!("Сервис не принял ключ ({status}).", status = answer.status().as_u16()))
        }
        Ok(answer) => Some(crate::tr!("Сервис ответил {status}.", status = answer.status().as_u16())),
        Err(error) => Some(crate::tr!("Сервис не отвечает: {error}", error = error)),
    }
}

/// The station's recognition chain asks the cloud through it (src/home_server/chain.rs).
pub trait CloudHearing: Send + Sync {
    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>>;
}

impl CloudHearing for CloudSpeech {
    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        CloudSpeech::recognizer(self)
    }
}

/// One recognizer, set once (tests, a chain without the panel).
impl CloudHearing for Option<Arc<dyn SpeechRecognizer>> {
    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
    pub service: Service,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// It hears (speaks) now: set up, with its key.
    pub ready: bool,
    /// Its key is in the safe.
    pub key_set: bool,
    /// The key is the brain's too (OpenAI, Groq).
    pub key_shared: bool,
    /// Set up in the .env, not in the panel.
    pub from_env: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub openai_hearing_model: &'static str,
    pub groq_hearing_model: &'static str,
    pub openai_voice_model: &'static str,
    pub openai_voice: &'static str,
    pub openai_voices: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSpeechView {
    /// This gateway hears and speaks through it (the station's); a computer's has its own models.
    pub active: bool,
    pub hearing: PartView,
    pub voice: PartView,
    pub defaults: Defaults,
}

/// `PUT /api/v1/speech/cloud`: one part.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub part: Part,
    pub service: Service,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CheckBody {
    part: Part,
}

#[derive(Debug, Clone, Serialize)]
struct CheckView {
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/speech/cloud", get(view).put(change)).route("/api/v1/speech/cloud/check", post(check))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<CloudSpeechView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.cloud_speech.view()))
}

async fn change(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<CloudSpeechView> {
    caller.permitted(MODELS_MANAGE)?;
    if !app.cloud_speech.active() {
        return Err(ApiError::conflict(crate::tr!(
            "Облачный слух и голос настраиваются на колонке: компьютер слышит и говорит своими моделями."
        )));
    }
    let change: Change = serde_json::from_value(body).map_err(|error| ApiError::bad_request(error.to_string()))?;
    app.cloud_speech.set(&change, caller.actor())?;
    tracing::info!(target: "speech", "cloud {}: {}", change.part.as_str(), change.service.title());
    Ok(ok(app.cloud_speech.view()))
}

async fn check(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<CheckView> {
    caller.permitted(CONNECTORS_VIEW)?;
    let body: CheckBody = serde_json::from_value(body).map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(ok(CheckView { problem: app.cloud_speech.check(body.part).await }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::storage::Db;

    fn cloud(pairs: &[(&str, &str)], active: bool) -> CloudSpeech {
        let config = Config::for_tests(&Env::from_pairs(pairs.iter().copied()));
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        CloudSpeech::new(settings, secrets, config.speech.asr, config.gateway.tts.own, active)
    }

    fn change(part: Part, service: Service) -> Change {
        Change { part, service, model: None, voice: None, url: None, key: None }
    }

    #[test]
    fn without_the_panel_the_envs_cloud_is_the_way_and_a_local_server_is_none() {
        let local = cloud(&[("ASR_BASE_URL", "http://127.0.0.1:8090/v1")], true);
        assert!(!local.hears() && !local.speaks());
        let env = cloud(
            &[
                ("ASR_BASE_URL", "https://api.groq.com/openai/v1"),
                ("TTS_PROVIDER", "mixed"),
                ("TTS_OWN_URL", "https://api.openai.com/v1"),
            ],
            true,
        );
        assert!(env.hears() && env.speaks());
        assert!(env.view().hearing.from_env);
    }

    #[test]
    fn one_openai_key_gives_hearing_and_voice_and_it_is_the_brains() {
        let cloud = cloud(&[], true);
        cloud.set(&change(Part::Hearing, Service::Openai), None).unwrap();
        assert!(!cloud.hears(), "no key yet");
        cloud.set(&Change { key: Some("sk-test".into()), ..change(Part::Voice, Service::Openai) }, None).unwrap();
        assert_eq!(cloud.secrets.get("llm:openai:key").as_deref(), Some("sk-test"), "the brain's key");
        assert!(cloud.hears() && cloud.speaks(), "the same key hears and speaks");
        let hearing = cloud.hearing_config().unwrap();
        assert_eq!((hearing.base_url.as_str(), hearing.model.as_str()), (OPENAI_URL, OPENAI_HEARING_MODEL));
        let voice = cloud.voice_config().unwrap();
        assert_eq!(
            (voice.model.as_str(), voice.voice.as_str(), voice.format.as_str()),
            (OPENAI_VOICE_MODEL, OPENAI_VOICE, "opus")
        );
    }

    #[test]
    fn a_change_builds_the_recognizer_again_and_off_is_none() {
        let cloud = cloud(&[], true);
        cloud.secrets.set("llm:groq:key", Some("gsk")).unwrap();
        cloud.set(&change(Part::Hearing, Service::Groq), None).unwrap();
        let first = cloud.recognizer().unwrap();
        assert!(Arc::ptr_eq(&first, &cloud.recognizer().unwrap()), "kept while nothing changed");
        assert!(first.description().contains(GROQ_HEARING_MODEL));
        cloud
            .set(&Change { model: Some("whisper-large-v3".into()), ..change(Part::Hearing, Service::Groq) }, None)
            .unwrap();
        assert!(cloud.recognizer().unwrap().description().contains("whisper-large-v3"));
        cloud.set(&change(Part::Hearing, Service::Off), None).unwrap();
        assert!(cloud.recognizer().is_none() && !cloud.hears());
    }

    #[test]
    fn groq_has_no_voice_and_an_own_address_needs_an_address() {
        let cloud = cloud(&[], true);
        assert!(cloud.set(&change(Part::Voice, Service::Groq), None).is_err());
        assert!(cloud.set(&change(Part::Hearing, Service::Custom), None).is_err());
        cloud
            .set(
                &Change { url: Some("https://speech.example.com/v1/".into()), ..change(Part::Voice, Service::Custom) },
                None,
            )
            .unwrap();
        assert_eq!(cloud.voice_config().unwrap().base_url, "https://speech.example.com/v1", "no key needed");
    }

    #[test]
    fn a_computers_gateway_does_not_use_it() {
        let cloud = cloud(&[("ASR_BASE_URL", "https://api.openai.com/v1")], false);
        assert!(cloud.recognizer().is_none() && !cloud.hears());
    }
}
