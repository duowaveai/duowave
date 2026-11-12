//! The home server (docs/plans/station-placement.md, section 5): the gateway on the station leans on the computer at
//! home for the heavy parts — recognition, our voice (the voice lab's too), a model of its own — while the computer is
//! up. On the computer: `/home/v1` (serve.rs), its own servers behind it, only from the home network and only with a
//! key that has the right `home.models`. On the station: the «Домашний сервер» connector (its address and that key),
//! a look every 15 s, and the ways down when it is away — recognition from the computer to the cloud to Yandex, the
//! voice from the computer to the cloud to Alice's (chain.rs). The gateway's own switch out of «Свой» and back is
//! here too (section 8): two looks in a row without an answer and «Свой» gives way to «Шлюз»; a minute of answers and
//! it comes back — only while the switch was the gateway's own. The computer's Telegram bot passes the station's
//! «передай …» on (messenger.rs): each look also reads the names of its contacts; and the bot's messages from them
//! come back to the station's `/api/v1/home-server/message`.

pub mod chain;
pub mod messenger;
#[cfg(feature = "pc")]
pub mod serve;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::config::{AsrConfig, Config, OwnSpeechConfig};
use crate::core::availability::HomeServer;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::gateway::mode::ModeService;
use crate::platform::http::ApiError;
use crate::secrets::{SecretMask, SecretsService};
use crate::settings::definitions::{HomeServerSetting, PanelMode, CONNECTOR_HOME_SERVER};
use crate::settings::SettingsService;
use crate::speech::asr::openai_transcriber::OpenAiTranscriber;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::tts::own_speech::OwnSpeech;
use crate::utils::time::now_iso;

/// The key the station reaches its home server with.
pub const KEY_SECRET: &str = "home-server:key";
/// What the station's .env last brought (a digest of its address and key): a new one — a computer's move wrote it —
/// is taken at the start; the same one leaves the panel's changes and its removal be.
const FROM_ENV_SECRET: &str = "home-server:from-env";
pub const CHECK_EVERY: Duration = Duration::from_secs(15);
/// Looks in a row without an answer before the computer counts as away (~30 s).
pub const DOWN_AFTER: u32 = 2;
/// How long the computer answers before «Свой» comes back by itself.
pub const RETURN_AFTER: Duration = Duration::from_secs(60);
pub const AWAY: &str = "компьютер не на связи";
pub const BACK: &str = "компьютер снова на связи";
/// «Что даю колонке» without hearing or voice, and no cloud for it in the station's .env: «Свой» has no Yandex to
/// fall back on, so it gives way to «Шлюз» as when the computer is away. The panel says «Сейчас «Шлюз»: слух
/// компьютера выключен с … Вернусь в «Свой», когда он появится».
pub const NO_HEARING: &str = "слух компьютера выключен";
pub const NO_VOICE: &str = "голос компьютера выключен";
/// The part given again: «Свой» comes back by itself.
pub const GIVEN: &str = "компьютер снова даёт колонке слух и голос";
/// «Модель» off while «Мозг» on the station answers with the computer's model: the station would go on asking the
/// computer (the provider cannot leave while a choice answers with it), so the switch is refused.
pub const MODEL_ANSWERS: &str =
    "Модель компьютера сейчас отвечает в «Мозге» колонки — сначала выберите там другую модель.";

#[derive(Debug, Clone, Default)]
struct Watch {
    /// Its last state: None — not looked at yet.
    up: Option<bool>,
    since: Option<String>,
    failures: u32,
    up_since: Option<Instant>,
    error: Option<String>,
}

/// The connection and its state, for both panels.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeServerStatus {
    /// The address and the key are there.
    pub configured: bool,
    /// And the station leans on the computer: «ПК помогает колонке».
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub key: SecretMask,
    pub state: HomeServer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the station does not take from the computer: "asr", "tts", "llm", "messages".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

/// What the computer can give the station: hearing, voice, the model, and its Telegram bot for «передай …».
pub const SERVES: &[&str] = &["asr", "tts", "llm", "messages"];

/// The computer's Telegram bot as the station last saw it (`/home/v1/messenger/contacts`). Kept while the computer
/// is away: «передай маме…» is still ours then, and says the computer is away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Messenger {
    /// Not read yet, or the computer has no such route (an older one).
    #[default]
    Unknown,
    /// The computer has no bot set up.
    NoBot,
    /// The names of its contacts, as its bot has them: no chats.
    Contacts(Vec<String>),
}

/// The computer's side, built for an address and a key.
struct Built {
    url: String,
    key: String,
    recognizer: Arc<dyn SpeechRecognizer>,
    speech: Arc<OwnSpeech>,
}

pub struct HomeServerService {
    config: Arc<Config>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    watch: Mutex<Watch>,
    built: Mutex<Option<Built>>,
    modes: OnceLock<Arc<ModeService>>,
    messenger: Mutex<Messenger>,
}

impl HomeServerService {
    /// The connection the station's .env brought (HOME_SERVER_URL, HOME_SERVER_KEY) is stored when the .env brings a
    /// new one: a computer's move writes it, and an older connection kept in the database (a gateway removed with its
    /// data kept, then put back) must not win over it (live, 07.11). The same .env again leaves the panel's changes and
    /// its removal be.
    pub fn new(
        config: Arc<Config>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
    ) -> Arc<HomeServerService> {
        let service = Arc::new(HomeServerService {
            config,
            settings,
            secrets,
            events,
            watch: Mutex::new(Watch::default()),
            built: Mutex::new(None),
            modes: OnceLock::new(),
            messenger: Mutex::new(Messenger::Unknown),
        });
        if let (Some(url), Some(key)) =
            (service.config.placement.home_server_url.clone(), service.config.placement.home_server_key.clone())
        {
            let brought = {
                use sha2::{Digest, Sha256};
                hex::encode(Sha256::digest(format!("{url}\n{key}").as_bytes()))
            };
            if service.secrets.get(FROM_ENV_SECRET).as_deref() != Some(brought.as_str()) {
                let _ = service.settings.set(
                    &CONNECTOR_HOME_SERVER,
                    &HomeServerSetting { url, enabled: true, without: Vec::new() },
                    None,
                );
                let _ = service.secrets.set(KEY_SECRET, Some(&key));
                let _ = service.secrets.set(FROM_ENV_SECRET, Some(&brought));
            }
        }
        service
    }

    pub fn set_modes(&self, modes: Arc<ModeService>) {
        let _ = self.modes.set(modes);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Watch> {
        self.watch.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The computer's address as the station knows it, whether it leans on it or not.
    pub fn known(&self) -> Option<HomeServerSetting> {
        self.settings.get(&CONNECTOR_HOME_SERVER, None)
    }

    /// The address and the key, when the connection is set up and on («ПК помогает колонке»).
    pub fn configured(&self) -> Option<(String, String)> {
        let setting = self.known().filter(|setting| setting.enabled)?;
        Some((setting.url, self.secrets.get(KEY_SECRET)?))
    }

    pub fn state(&self) -> HomeServer {
        if self.configured().is_none() {
            return HomeServer::None;
        }
        match self.lock().up {
            Some(true) => HomeServer::Up,
            _ => HomeServer::Down,
        }
    }

    pub fn status(&self) -> HomeServerStatus {
        let known = self.known();
        let watch = self.lock().clone();
        HomeServerStatus {
            configured: known.is_some() && self.secrets.has(KEY_SECRET),
            enabled: known.as_ref().is_some_and(|setting| setting.enabled),
            without: known.as_ref().map(|setting| setting.without.clone()).unwrap_or_default(),
            url: known.map(|setting| setting.url),
            key: self.secrets.mask(KEY_SECRET),
            state: self.state(),
            since: watch.since,
            error: watch.error,
        }
    }

    /// The panel's connector: the computer's address and the key made on it («Доступ» → «Ключи», right
    /// `home.models`); a key not given is kept. None removes the connection.
    pub fn set(
        &self,
        url: Option<&str>,
        key: Option<&str>,
        actor: Option<Actor>,
    ) -> Result<HomeServerStatus, ApiError> {
        match url.map(str::trim).filter(|url| !url.is_empty()) {
            None => {
                self.settings.forget(&CONNECTOR_HOME_SERVER)?;
                self.secrets.delete(KEY_SECRET)?;
            }
            Some(url) => {
                let parsed =
                    url::Url::parse(url).map_err(|_| ApiError::bad_request("Адрес компьютера: http://адрес:порт"))?;
                if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                    return Err(ApiError::bad_request("Адрес компьютера: http://адрес:порт"));
                }
                if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
                    self.secrets.set(KEY_SECRET, Some(key))?;
                }
                if !self.secrets.has(KEY_SECRET) {
                    return Err(ApiError::bad_request(
                        "Нужен ключ с компьютера: «Доступ» → «Ключи», право «модели этого компьютера».",
                    ));
                }
                let url = url.trim_end_matches('/').to_string();
                let enabled = self.known().is_none_or(|setting| setting.enabled);
                self.settings.set(
                    &CONNECTOR_HOME_SERVER,
                    &HomeServerSetting {
                        url,
                        enabled,
                        without: self.known().map(|setting| setting.without).unwrap_or_default(),
                    },
                    actor.clone(),
                )?;
            }
        }
        *self.lock() = Watch::default();
        *self.built.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        // Another computer, or none: its bot is read again.
        *self.messenger.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Messenger::Unknown;
        self.events.emit(GatewayEvent::HomeServerChanged { configured: self.configured().is_some(), actor });
        Ok(self.status())
    }

    /// The pair's way to the connection (the computer's panel and its watch, the station's panel): a new address — the
    /// computer's moved on the home network; a new key; the switch «ПК помогает колонке». What is not given stays.
    pub fn link(
        &self,
        url: Option<&str>,
        key: Option<&str>,
        enabled: Option<bool>,
        without: Option<Vec<String>>,
        actor: Option<Actor>,
    ) -> Result<HomeServerStatus, ApiError> {
        let url = url.map(str::trim).filter(|url| !url.is_empty());
        let key = key.map(str::trim).filter(|key| !key.is_empty());
        if url.is_some() || key.is_some() {
            let url = match url {
                Some(url) => url.to_string(),
                None => self.known().map(|setting| setting.url).ok_or_else(|| {
                    ApiError::bad_request(
                        "Адреса компьютера ещё нет: впишите его или включите помощь из пульта компьютера.",
                    )
                })?,
            };
            self.set(Some(&url), key, actor.clone())?;
        }
        if let Some(on) = enabled {
            let Some(mut setting) = self.known().filter(|_| self.secrets.has(KEY_SECRET)) else {
                return Err(ApiError::bad_request(
                    "Колонка ещё не знает компьютер: включите «Помогать колонке» в пульте компьютера — он пришлёт адрес и ключ.",
                ));
            };
            if setting.enabled != on {
                setting.enabled = on;
                self.settings.set(&CONNECTOR_HOME_SERVER, &setting, actor.clone())?;
                *self.lock() = Watch::default();
                self.events.emit(GatewayEvent::HomeServerChanged { configured: on, actor: actor.clone() });
                if !on {
                    self.leave_svoi(AWAY);
                }
            }
        }
        // «Что даю колонке»: слух, голос, модель и сообщения по отдельности.
        if let Some(without) = without {
            let mut without: Vec<String> = without.into_iter().filter(|what| SERVES.contains(&what.as_str())).collect();
            without.sort();
            without.dedup();
            let Some(mut setting) = self.known() else {
                return Err(ApiError::bad_request(
                    "Колонка ещё не знает компьютер: сначала включите «Помогать колонке» в пульте компьютера.",
                ));
            };
            if setting.without != without {
                setting.without = without;
                self.settings.set(&CONNECTOR_HOME_SERVER, &setting, actor.clone())?;
                self.events.emit(GatewayEvent::HomeServerChanged { configured: setting.enabled, actor });
                // In «Свой» a part not given may leave the station deaf or mute: it goes as when the computer is away.
                self.keep_svoi();
            }
        }
        Ok(self.status())
    }

    /// Whether the station would take the model from the computer no more: `without` names it, the stored one not.
    pub fn takes_model_away(&self, without: &[String]) -> bool {
        without.iter().any(|what| what == "llm")
            && self.known().is_some_and(|setting| !setting.without.iter().any(|off| off == "llm"))
    }

    /// What «Свой» would lack now: the computer's hearing or voice not taken («Что даю колонке») and no cloud for it
    /// in the station's .env — without Yandex there it would hear or say nothing. On the station only (a computer's
    /// gateway hears and speaks itself), and only while it leans on the computer: with the help off «Свой» has gone
    /// already, as with a computer away.
    pub fn svoi_lacks(&self) -> Option<&'static str> {
        if !crate::core::build::Profile::for_role(self.config.placement.role.as_deref()).station {
            return None;
        }
        self.configured()?;
        let cloud = |url: &str| !crate::operations::networks::is_local_url(Some(url));
        let asr = &self.config.speech.asr;
        if !self.serves("asr") && !(asr.enabled && cloud(&asr.base_url)) {
            return Some(NO_HEARING);
        }
        if !self.serves("tts") && !self.config.gateway.tts.own.as_ref().is_some_and(|own| cloud(&own.base_url)) {
            return Some(NO_VOICE);
        }
        None
    }

    /// Whether the station takes this from the computer now: "asr", "tts", "llm", "messages".
    pub fn serves(&self, what: &str) -> bool {
        self.known().is_some_and(|setting| setting.enabled && !setting.without.iter().any(|off| off == what))
    }

    /// The computer's recognition, while it is up and the station takes it.
    pub fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        if self.state() != HomeServer::Up || !self.serves("asr") {
            return None;
        }
        self.built().map(|built| built.0)
    }

    /// The computer's voice — its own choice, a voice lab's voice too — while it is up and the station takes it.
    pub fn speech(&self) -> Option<Arc<OwnSpeech>> {
        if self.state() != HomeServer::Up || !self.serves("tts") {
            return None;
        }
        self.built().map(|built| built.1)
    }

    fn built(&self) -> Option<(Arc<dyn SpeechRecognizer>, Arc<OwnSpeech>)> {
        let (url, key) = self.configured()?;
        let mut built = self.built.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if built.as_ref().is_none_or(|built| built.url != url || built.key != key) {
            let asr = &self.config.speech.asr;
            let recognizer = OpenAiTranscriber::new(AsrConfig {
                enabled: true,
                base_url: format!("{url}/home/v1/asr"),
                api_key: Some(key.clone()),
                model: "home".into(),
                language: asr.language.clone(),
                prompt: asr.prompt.clone(),
                timeout_ms: asr.timeout_ms,
                wake_words: asr.wake_words.clone(),
                local: None,
            });
            let speech = OwnSpeech::new(OwnSpeechConfig {
                base_url: format!("{url}/home/v1/tts"),
                api_key: Some(key.clone()),
                model: "home".into(),
                voice: "home".into(),
                // The computer answers with Ogg/Opus, ready for the station.
                format: "opus".into(),
                timeout_ms: self.config.gateway.tts.own.as_ref().map(|own| own.timeout_ms).unwrap_or(15_000),
            });
            *built = Some(Built { url, key, recognizer: Arc::new(recognizer), speech: Arc::new(speech) });
        }
        built.as_ref().map(|built| (built.recognizer.clone(), built.speech.clone()))
    }

    /// Every 15 s while the connection is set up.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                if me.configured().is_some() {
                    me.check().await;
                }
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    /// One look at the computer: `/home/v1/status` with the key.
    pub async fn check(&self) {
        let Some((url, key)) = self.configured() else { return };
        let answer = crate::utils::http::client()
            .get(format!("{url}/home/v1/status"))
            .bearer_auth(&key)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let outcome = match answer {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) if response.status().as_u16() == 401 || response.status().as_u16() == 403 => {
                Err("компьютер не принимает ключ: сделайте новый в «Доступ» → «Ключи»".to_string())
            }
            Ok(response) => Err(format!("компьютер ответил {}", response.status().as_u16())),
            Err(error) if error.is_timeout() => Err("компьютер не ответил вовремя".into()),
            Err(_) => Err(AWAY.into()),
        };
        let up = outcome.is_ok();
        self.seen(&url, outcome);
        if up && self.serves("messages") {
            self.read_contacts(&url, &key).await;
        }
    }

    /// The computer's Telegram bot as the station last saw it.
    pub fn messenger(&self) -> Messenger {
        self.messenger.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The names of the computer's contacts, or that it has no bot. A miss keeps what was seen.
    async fn read_contacts(&self, url: &str, key: &str) {
        let answer = crate::utils::http::client()
            .get(format!("{url}/home/v1/messenger/contacts"))
            .bearer_auth(key)
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        let seen = match answer {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(body) => Messenger::Contacts(
                    body.get("contacts")
                        .and_then(Value::as_array)
                        .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default(),
                ),
                Err(_) => return,
            },
            Ok(response) if response.status().as_u16() == 503 => Messenger::NoBot,
            // A computer of an older version: no messages through it.
            Ok(response) if response.status().as_u16() == 404 => Messenger::Unknown,
            Ok(response) => {
                tracing::debug!("home server: its contacts: HTTP {}", response.status().as_u16());
                return;
            }
            Err(error) => {
                tracing::debug!("home server: its contacts: {error}");
                return;
            }
        };
        *self.messenger.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = seen;
    }

    /// A look's outcome: the state, the events, the gateway's own switch.
    fn seen(&self, url: &str, outcome: Result<(), String>) {
        let (changed, up) = {
            let mut watch = self.lock();
            match outcome {
                Ok(()) => {
                    watch.failures = 0;
                    watch.error = None;
                    let changed = watch.up != Some(true);
                    if changed {
                        watch.up = Some(true);
                        watch.since = Some(now_iso());
                        watch.up_since = Some(Instant::now());
                    }
                    (changed, true)
                }
                Err(error) => {
                    watch.failures += 1;
                    watch.error = Some(error);
                    // The first look decides at once; later, two misses in a row.
                    let away = watch.up.is_none() || watch.failures >= DOWN_AFTER;
                    let changed = away && watch.up != Some(false);
                    if changed {
                        watch.up = Some(false);
                        watch.since = Some(now_iso());
                        watch.up_since = None;
                    }
                    (changed, false)
                }
            }
        };
        if changed {
            if up {
                tracing::info!("home server: {url} is up");
                self.events.emit(GatewayEvent::HomeServerUp { url: url.to_string() });
            } else {
                let reason = self.lock().error.clone().unwrap_or_else(|| AWAY.into());
                tracing::warn!("home server: {url} is away: {reason}");
                self.events.emit(GatewayEvent::HomeServerDown { url: url.to_string(), reason });
                self.leave_svoi(AWAY);
            }
        }
        if up {
            self.keep_svoi();
        }
    }

    /// The computer went, or «Свой» would be deaf or mute without a part of it: «Свой» gives way to «Шлюз», kept as
    /// the gateway's own switch, with why.
    fn leave_svoi(&self, reason: &str) {
        let Some(modes) = self.modes.get() else { return };
        match modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, reason) {
            Ok(true) => tracing::warn!("mode: «Свой» → «Шлюз» by itself: {reason}"),
            Ok(false) => {}
            Err(error) => tracing::warn!("mode: «Свой» not left: {}", error.message),
        }
    }

    /// With the computer up: «Свой» stays only with hearing and voice — the computer's or the cloud's. The mode switch
    /// does not offer it without them (the registry asks `own_lacks`); one kept from before gives way at the next
    /// look, and once they are given again it comes back, as after a computer away.
    fn keep_svoi(&self) {
        match self.svoi_lacks() {
            Some(reason) => self.leave_svoi(reason),
            None => self.maybe_return(),
        }
    }

    /// A minute of answers: «Свой» back, if the switch out of it was the gateway's own — never while it would lack
    /// hearing or voice there.
    fn maybe_return(&self) {
        let Some(modes) = self.modes.get() else { return };
        let long_enough = self.lock().up_since.is_some_and(|since| since.elapsed() >= RETURN_AFTER);
        if !long_enough || self.svoi_lacks().is_some() {
            return;
        }
        let left_for_a_part =
            modes.state().auto.is_some_and(|auto| auto.reason == NO_HEARING || auto.reason == NO_VOICE);
        let reason = if left_for_a_part { GIVEN } else { BACK };
        match modes.return_by_itself(reason) {
            Ok(true) => tracing::info!("mode: «Свой» is back by itself: {reason}"),
            Ok(false) => {}
            Err(error) => tracing::warn!("mode: «Свой» not back: {}", error.message),
        }
    }

    /// For tests: a minute passed since the computer came back.
    #[cfg(test)]
    fn age_up(&self, by: Duration) {
        let mut watch = self.lock();
        watch.up_since = watch.up_since.and_then(|since| since.checked_sub(by));
    }
}

impl crate::placement::HomeServerSource for HomeServerService {
    fn home_server(&self) -> HomeServer {
        self.state()
    }

    /// The registry's «Свой» knows it too: the switch refuses it with why, and the panel shows it off.
    fn own_lacks(&self) -> Option<&'static str> {
        self.svoi_lacks()
    }
}

impl crate::placement::service::HomeServerView for HomeServerService {
    fn view(&self) -> Option<Value> {
        let status = self.status();
        status.configured.then(|| {
            serde_json::json!({
                "url": status.url,
                "enabled": status.enabled,
                "state": status.state,
                "since": status.since,
                "error": status.error,
                "without": status.without,
            })
        })
    }
}

/// Our voice through the computer, for the voice service (src/speech/voice_service.rs).
impl crate::speech::voice_service::HomeVoice for HomeServerService {
    fn speech(&self) -> Option<Arc<OwnSpeech>> {
        HomeServerService::speech(self)
    }
}

/// The title of the computer's model among «Мозг»'s providers on the station.
pub const BRAIN_TITLE: &str = "Модель на компьютере";

/// «Мозг» on the station: the computer's own model is one of the providers while the home server is set up — added
/// with its address and key, changed with them, removed with the connection (kept while a choice answers with it).
/// The plugins' way to the computer (docs/plans/plugins.md, stage 7): a plugin with `prefer: home` runs there while
/// it is up.
impl crate::plugins::controller::HomeLink for HomeServerService {
    fn up(&self) -> Option<(String, String)> {
        (self.state() == HomeServer::Up).then(|| self.configured()).flatten()
    }
}

/// The computer's model among «Мозг»'s providers: added here, under its title, at the computer's `/home/v1/llm`.
fn computers(provider: &crate::dialog::brain::ProviderView) -> bool {
    provider.title == BRAIN_TITLE && provider.base_url.as_deref().is_some_and(|url| url.ends_with("/home/v1/llm"))
}

/// Whether «Мозг» on the station answers with the computer's model now: its fast, its smart or its backup pick is on
/// it.
pub fn brain_answers_with_computer(brain: &crate::dialog::brain::BrainService) -> bool {
    let view = brain.view();
    let on_computer = |id: &str| view.providers.iter().any(|provider| provider.id == id && computers(provider));
    let choice = &view.choice;
    choice.fast.iter().chain(choice.smart.iter()).chain(choice.backup.iter()).any(|pick| on_computer(&pick.provider))
}

/// «Что даю колонке» as the station's gateway takes it: «Модель» is not let go while «Мозг» answers with the
/// computer's model — the station would go on asking the computer (the provider stays while a choice answers with
/// it) and the switch would promise what it does not do. Hearing and voice go their own way (`link`).
pub fn check_parts(
    home: &HomeServerService,
    brain: &crate::dialog::brain::BrainService,
    without: &[String],
) -> Result<(), ApiError> {
    if home.takes_model_away(without) && brain_answers_with_computer(brain) {
        return Err(ApiError::conflict(MODEL_ANSWERS));
    }
    Ok(())
}

pub fn keep_brain_provider(home: &HomeServerService, brain: &crate::dialog::brain::BrainService) {
    use crate::dialog::brain::brain_service::ProviderInput;
    let view = brain.view();
    let ours = view.providers.iter().find(|provider| computers(provider));
    // The model off in «Что даю колонке»: the computer's model leaves the providers, as when there is no computer.
    match (home.configured().filter(|_| home.serves("llm")), ours) {
        (Some((url, key)), None) => {
            let input = ProviderInput(serde_json::Map::from_iter([
                ("preset".into(), serde_json::json!("custom-openai")),
                ("title".into(), serde_json::json!(BRAIN_TITLE)),
                ("baseUrl".into(), serde_json::json!(format!("{url}/home/v1/llm"))),
                ("key".into(), serde_json::json!(key)),
            ]));
            if let Err(error) = brain.add(&input, None) {
                tracing::warn!("home server: the computer's model is not among the providers: {}", error.message);
            }
        }
        (Some((url, key)), Some(provider)) if provider.base_url.as_deref() != Some(&format!("{url}/home/v1/llm")) => {
            let input = ProviderInput(serde_json::Map::from_iter([
                ("baseUrl".into(), serde_json::json!(format!("{url}/home/v1/llm"))),
                ("key".into(), serde_json::json!(key)),
            ]));
            let _ = brain.update(&provider.id, &input, None);
        }
        (None, Some(provider)) => {
            if let Err(error) = brain.remove(&provider.id, None) {
                tracing::info!("home server: the computer's model stays among the providers: {}", error.message);
            }
        }
        _ => {}
    }
}

/// `/api/v1/home-server`: the station's panel sets the connection up; `/home/v1` on the computer (serve.rs).
pub fn router() -> axum::Router<Arc<crate::app::App>> {
    use axum::routing::{get, post};
    let router = axum::Router::new()
        .route("/api/v1/home-server", get(routes::status).put(routes::set).delete(routes::remove))
        .route("/api/v1/home-server/check", post(routes::check_now))
        .route("/api/v1/home-server/link", post(routes::link))
        // The computer's bot's messages for the station (messenger.rs): a voice message is up to 2 MB.
        .route(
            messenger::MESSAGE_PATH,
            post(messenger::receive).layer(axum::extract::DefaultBodyLimit::max(messenger::BODY_LIMIT)),
        );
    #[cfg(feature = "pc")]
    let router = router.merge(serve::router()).route("/api/v1/home-server/served", get(routes::served));
    router
}

mod routes {
    use std::sync::Arc;

    use axum::extract::State;
    use serde_json::Value;

    use super::HomeServerStatus;
    use crate::app::App;
    use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
    use crate::platform::http::{created, ok, ApiResult, Body, Caller};

    /// The helper's «Что даю колонке»: how many times and how fast it gave the station hearing, voice and the model
    /// today.
    #[cfg(feature = "pc")]
    pub async fn served(caller: Caller) -> ApiResult<Value> {
        caller.permitted(crate::core::identity::permissions::STATION_VIEW)?;
        Ok(ok(super::serve::served_today()))
    }

    pub async fn status(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_VIEW)?;
        Ok(ok(app.home_server.status()))
    }

    pub async fn set(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_MANAGE)?;
        let url = body.get("url").and_then(Value::as_str);
        let key = body.get("key").and_then(Value::as_str);
        app.home_server.set(url, key, caller.actor())?;
        app.home_server.check().await;
        Ok(created(app.home_server.status()))
    }

    pub async fn remove(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_MANAGE)?;
        Ok(ok(app.home_server.set(None, None, caller.actor())?))
    }

    /// The address, the key, the switch: from the computer with the pair's key, or a person with the right.
    pub async fn link(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<HomeServerStatus> {
        crate::placement::peer::peer_or_permitted(&caller, app.placement.peer_key(), CONNECTORS_MANAGE)?;
        let without: Option<Vec<String>> = body
            .get("without")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect());
        // Before anything is stored: «Модель» stays while «Мозг» answers with it (the computer's panel says why).
        if let Some(without) = &without {
            super::check_parts(&app.home_server, &app.brain, without)?;
        }
        let status = app.home_server.link(
            body.get("url").and_then(Value::as_str),
            body.get("key").and_then(Value::as_str),
            body.get("enabled").and_then(Value::as_bool),
            without,
            caller.actor(),
        )?;
        if status.enabled {
            app.home_server.check().await;
        }
        Ok(ok(app.home_server.status()))
    }

    pub async fn check_now(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeServerStatus> {
        caller.permitted(CONNECTORS_VIEW)?;
        app.home_server.check().await;
        Ok(created(app.home_server.status()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::routing::get;
    use axum::Router;

    use super::*;
    use crate::config::Env;
    use crate::core::build::ModeGate;
    use crate::speech::VoiceService;
    use crate::storage::Db;

    /// The station with its home server: «Свой» only while the server is up — as the registry says.
    struct Gate(Arc<HomeServerService>);

    impl ModeGate for Gate {
        fn check(&self, mode: PanelMode) -> Result<(), String> {
            if mode == PanelMode::Own && self.0.state() != HomeServer::Up {
                return Err("«Своё» сейчас не включить: компьютер не на связи.".into());
            }
            Ok(())
        }
        fn absent(&self, _mode: PanelMode) -> Option<String> {
            None
        }
    }

    async fn computer(up: Arc<AtomicBool>) -> String {
        let router = Router::new().route(
            "/home/v1/status",
            get(move |headers: axum::http::HeaderMap| {
                let up = up.load(Ordering::Relaxed);
                async move {
                    if !up {
                        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                    }
                    if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("Bearer vgw_home") {
                        return Err(axum::http::StatusCode::UNAUTHORIZED);
                    }
                    Ok(axum::Json(serde_json::json!({"asr": true, "tts": true})))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[test]
    fn a_new_address_in_the_stations_env_wins_over_an_old_one_in_its_database_the_same_one_leaves_the_panels() {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let start = |url: &str| {
            let config = Arc::new(Config::for_tests(&Env::from_pairs([
                ("HOME_SERVER_URL", url),
                ("HOME_SERVER_KEY", "vgw_home"),
            ])));
            HomeServerService::new(config, settings.clone(), secrets.clone(), EventBus::new())
        };
        // Kept from an earlier install: a computer that is not there any more.
        settings
            .set(
                &CONNECTOR_HOME_SERVER,
                &HomeServerSetting { url: "http://192.0.2.10:8191".into(), enabled: true, without: Vec::new() },
                None,
            )
            .unwrap();
        let home = start("http://192.0.2.10");
        assert_eq!(home.configured().map(|(url, _)| url).as_deref(), Some("http://192.0.2.10"), "the move's address");
        settings
            .set(
                &CONNECTOR_HOME_SERVER,
                &HomeServerSetting { url: "http://192.0.2.11".into(), enabled: true, without: Vec::new() },
                None,
            )
            .unwrap();
        let home = start("http://192.0.2.10");
        assert_eq!(home.configured().map(|(url, _)| url).as_deref(), Some("http://192.0.2.11"), "the panel's stays");
    }

    #[tokio::test]
    async fn the_computer_goes_svoi_gives_way_it_comes_back_and_svoi_follows_after_a_minute() {
        let up = Arc::new(AtomicBool::new(true));
        let url = computer(up.clone()).await;
        let config = Arc::new(Config::for_tests(&Env::from_pairs([
            ("HOME_SERVER_URL", url.as_str()),
            ("HOME_SERVER_KEY", "vgw_home"),
            ("UNIPROXY_MODE", "gateway"),
        ])));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let home = HomeServerService::new(config.clone(), settings.clone(), secrets, events.clone());
        assert_eq!(home.configured().map(|(url, _)| url), Some(url.clone()), "the station's .env, stored once");
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let modes =
            Arc::new(ModeService::new(config.gateway.mode, settings, events, voice, Arc::new(Gate(home.clone()))));
        home.set_modes(modes.clone());

        home.check().await;
        assert_eq!(home.state(), HomeServer::Up);
        assert!(home.recognizer().is_some() && home.speech().is_some(), "its voice and its ears, while up");
        modes.switch(Some(&serde_json::json!("own")), None).unwrap();

        up.store(false, Ordering::Relaxed);
        home.check().await;
        assert_eq!(home.state(), HomeServer::Up, "one miss is not away yet");
        home.check().await;
        assert_eq!(home.state(), HomeServer::Down);
        assert!(home.speech().is_none());
        let view = modes.view();
        assert_eq!((view.mode, view.auto.map(|auto| auto.reason)), (PanelMode::Gateway, Some(AWAY.to_string())));

        up.store(true, Ordering::Relaxed);
        home.check().await;
        assert_eq!((home.state(), modes.current()), (HomeServer::Up, PanelMode::Gateway), "not before a minute");
        home.age_up(RETURN_AFTER);
        home.check().await;
        assert_eq!(modes.current(), PanelMode::Own, "back, the switch was its own");

        // A key the computer no longer takes is said so.
        home.set(Some(&url), Some("vgw_revoked"), None).unwrap();
        home.check().await;
        assert_eq!(home.state(), HomeServer::Down);
        assert!(home.status().error.unwrap_or_default().contains("ключ"));
        home.set(None, None, None).unwrap();
        assert_eq!(home.state(), HomeServer::None);
    }

    #[test]
    fn the_switch_keeps_the_address_and_the_key_off_the_station_works_alone_on_it_leans_on_the_computer() {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let config = Arc::new(Config::for_tests(&Env::from_pairs([
            ("HOME_SERVER_URL", "http://192.0.2.10"),
            ("HOME_SERVER_KEY", "vgw_home"),
        ])));
        let home = HomeServerService::new(config, settings, secrets, EventBus::new());
        assert!(home.configured().is_some(), "the move brought it on");

        let status = home.link(None, None, Some(false), None, None).unwrap();
        assert!(status.configured && !status.enabled, "kept, off");
        assert_eq!(status.url.as_deref(), Some("http://192.0.2.10"));
        assert_eq!(home.configured(), None, "not leaned on");
        assert_eq!(home.state(), HomeServer::None);

        // The computer moved on the home network: the new address, the switch stays off.
        let status = home.link(Some("http://192.0.2.35:3000"), None, None, None, None).unwrap();
        assert_eq!((status.url.as_deref(), status.enabled), (Some("http://192.0.2.35:3000"), false));

        let status = home.link(None, None, Some(true), None, None).unwrap();
        assert!(status.enabled);
        assert_eq!(home.configured(), Some(("http://192.0.2.35:3000".into(), "vgw_home".into())));
        // «Что даю колонке»: без модели — слух и голос остаются.
        let status = home.link(None, None, None, Some(vec!["llm".into(), "bogus".into()]), None).unwrap();
        assert_eq!(status.without, vec!["llm"]);
        assert!(home.serves("asr") && home.serves("tts") && !home.serves("llm"));
    }

    /// The station leaning on the computer at `url`, its modes asking the gate above; `extra` — more of its .env.
    fn station_on(url: &str, extra: &[(&str, &str)]) -> (Arc<HomeServerService>, Arc<ModeService>) {
        let mut pairs = vec![
            ("HOME_SERVER_URL", url),
            ("HOME_SERVER_KEY", "vgw_home"),
            ("UNIPROXY_MODE", "gateway"),
            ("DUOWAVE_ROLE", "station"),
        ];
        pairs.extend_from_slice(extra);
        let config = Arc::new(Config::for_tests(&Env::from_pairs(pairs)));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let home = HomeServerService::new(config.clone(), settings.clone(), secrets, events.clone());
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let modes =
            Arc::new(ModeService::new(config.gateway.mode, settings, events, voice, Arc::new(Gate(home.clone()))));
        home.set_modes(modes.clone());
        (home, modes)
    }

    /// Review G2: in «Свой» there is no Yandex — without the computer's hearing or voice and no cloud for it the
    /// station would hear or say nothing. It gives way to «Шлюз» as when the computer is away (Yandex hears, Alice
    /// speaks), says why, and comes back once the part is given again.
    #[tokio::test]
    async fn svoi_gives_way_while_the_computers_hearing_or_voice_is_not_given_and_comes_back_with_it() {
        let up = Arc::new(AtomicBool::new(true));
        let url = computer(up.clone()).await;
        let (home, modes) = station_on(&url, &[]);
        home.check().await;
        home.age_up(RETURN_AFTER);
        modes.switch(Some(&serde_json::json!("own")), None).unwrap();

        // «Слух» off, the station's recognition is the build's local default: no cloud to hear.
        home.link(None, None, None, Some(vec!["asr".into()]), None).unwrap();
        let view = modes.view();
        assert_eq!((view.mode, view.auto.map(|auto| auto.reason)), (PanelMode::Gateway, Some(NO_HEARING.to_string())));
        home.check().await;
        assert_eq!(modes.current(), PanelMode::Gateway, "not back while it would be deaf");

        // Given again: back at once, the computer has answered for a minute.
        home.link(None, None, None, Some(Vec::new()), None).unwrap();
        assert_eq!(modes.current(), PanelMode::Own);

        // «Голос» off: mute without a cloud voice — the same way.
        home.link(None, None, None, Some(vec!["tts".into()]), None).unwrap();
        assert_eq!(modes.view().auto.map(|auto| auto.reason), Some(NO_VOICE.to_string()));
        // A person switches to «Свой» all the same (the switch asks only whether the computer is up): the next look
        // gives way again.
        modes.switch(Some(&serde_json::json!("own")), None).unwrap();
        home.check().await;
        let view = modes.view();
        assert_eq!((view.mode, view.auto.map(|auto| auto.reason)), (PanelMode::Gateway, Some(NO_VOICE.to_string())));

        // The voice given back, only «Модель» off: hearing and voice are there, «Свой» is back.
        home.link(None, None, None, Some(vec!["llm".into()]), None).unwrap();
        assert_eq!((modes.current(), home.svoi_lacks()), (PanelMode::Own, None));
    }

    #[tokio::test]
    async fn svoi_stays_without_the_computers_hearing_when_a_cloud_hears_and_on_a_computer_it_is_never_asked() {
        let up = Arc::new(AtomicBool::new(true));
        let url = computer(up.clone()).await;
        let (home, modes) = station_on(&url, &[("ASR_BASE_URL", "https://api.groq.com/openai/v1")]);
        home.check().await;
        modes.switch(Some(&serde_json::json!("own")), None).unwrap();
        home.link(None, None, None, Some(vec!["asr".into()]), None).unwrap();
        assert_eq!(modes.current(), PanelMode::Own, "the cloud hears");
        assert_eq!(home.svoi_lacks(), None);
        home.link(None, None, None, Some(vec!["asr".into(), "tts".into()]), None).unwrap();
        assert_eq!((modes.current(), home.svoi_lacks()), (PanelMode::Gateway, Some(NO_VOICE)), "no cloud voice");

        // A computer's gateway hears and speaks itself: its parts never send «Свой» away.
        let (home, modes) = station_on(&url, &[("DUOWAVE_ROLE", "pc")]);
        home.check().await;
        modes.switch(Some(&serde_json::json!("own")), None).unwrap();
        home.link(None, None, None, Some(vec!["asr".into(), "tts".into()]), None).unwrap();
        assert_eq!((modes.current(), home.svoi_lacks()), (PanelMode::Own, None));
    }

    /// Review G3: «Модель» off while «Мозг» on the station answers with the computer's model would change nothing —
    /// the provider stays while it answers — so the switch is refused with why; otherwise the model goes, and the
    /// provider with it.
    #[test]
    fn the_model_is_not_let_go_while_the_stations_brain_answers_with_it() {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let config = Arc::new(Config::for_tests(&Env::from_pairs([
            ("HOME_SERVER_URL", "http://192.0.2.10"),
            ("HOME_SERVER_KEY", "vgw_home"),
            ("DIALOG_PROVIDER", "none"),
        ])));
        let home = HomeServerService::new(config.clone(), settings.clone(), secrets.clone(), events.clone());
        let brain =
            crate::dialog::brain::BrainService::new(config.dialog.clone(), db, settings, secrets, events).unwrap();
        keep_brain_provider(&home, &brain);
        let ours = brain.view().providers.into_iter().find(computers).expect("the computer's model is there").id;
        let pick = |fast: serde_json::Value| {
            brain.choose(&serde_json::Map::from_iter([("fast".to_string(), fast)]), None).unwrap();
        };

        pick(serde_json::json!({"provider": ours, "model": "home"}));
        assert!(brain_answers_with_computer(&brain));
        let refused = check_parts(&home, &brain, &["llm".into()]).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.message.as_str()), (409, MODEL_ANSWERS));
        assert!(check_parts(&home, &brain, &["asr".into(), "tts".into()]).is_ok(), "hearing and voice go their way");

        // Another model answers: «Модель» goes, the provider with it.
        pick(serde_json::Value::Null);
        assert!(check_parts(&home, &brain, &["llm".into()]).is_ok());
        home.link(None, None, None, Some(vec!["llm".into()]), None).unwrap();
        keep_brain_provider(&home, &brain);
        assert!(!brain.view().providers.iter().any(computers));
        // Off already: nothing more is asked of the other parts.
        assert!(check_parts(&home, &brain, &["asr".into(), "llm".into()]).is_ok());
    }

    #[test]
    fn the_switch_cannot_turn_on_a_computer_the_station_never_heard_of() {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let home = HomeServerService::new(
            Arc::new(Config::for_tests(&Env::from_pairs::<[(&str, &str); 0], &str, &str>([]))),
            settings,
            secrets,
            EventBus::new(),
        );
        assert!(home.link(None, None, Some(true), None, None).is_err());
        assert!(
            home.link(Some("http://192.0.2.10"), None, None, None, None).is_err(),
            "no key: the computer must bring one"
        );
    }
}
