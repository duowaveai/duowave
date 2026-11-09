//! The connectors the panel sets up: Home Assistant, Telegram, LiveKit, SIP, the weather's place and the news feeds.
//! Their plain fields are one setting each (settings/definitions.rs), their tokens, passwords and keys are secrets
//! (src/secrets). Every change applies at once: the modules read the values on each use (the skills, MCP, the
//! alerts) or hear of the change (the bot and the bridges of the calls, the Host check), and it is audited as
//! `connector.changed` with the fields' names, never their values.
//!
//! The environment only seeds them. At a start where the database has no row for a connector (a fresh install, the
//! first start of this version), its .env values move in once, marked as still the seeded ones; from then on its
//! .env lines are not read, and the log and the panel name the lines left. A connector cleared in the panel keeps
//! an empty row, so .env does not bring it back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::host_of;
use crate::config::{ConnectorsSeed, HomeAssistantConfig, LiveKitConfig, SipConfig, TelegramConfig};
use crate::core::connectors::ConnectorStatus;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::settings::SettingDefinition;
use crate::operations::networks::is_local_url;
use crate::platform::http::ApiError;
use crate::secrets::{SecretMask, SecretsService};
use crate::settings::definitions::{
    ContactEntry, HomeAssistantSetting, LiveKitSetting, NewsFeedEntry, NewsSetting, SipSetting, TelegramSetting,
    WeatherSetting, CONNECTOR_HOME_ASSISTANT, CONNECTOR_LIVEKIT, CONNECTOR_NEWS, CONNECTOR_SIP, CONNECTOR_TELEGRAM,
    CONNECTOR_WEATHER,
};
use crate::settings::SettingsService;
use crate::skills::lookups::news_catalog::{same_feed, NEWS_CATALOG};
use crate::utils::time::now_iso;

/// A connector the panel sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConnectorId {
    HomeAssistant,
    Telegram,
    LiveKit,
    Sip,
    Weather,
    News,
}

const HA_TOKEN: &str = "connector:home-assistant:token";
const TELEGRAM_TOKEN: &str = "connector:telegram:token";
const LIVEKIT_KEY: &str = "connector:livekit:key";
const LIVEKIT_SECRET: &str = "connector:livekit:secret";
const SIP_PASSWORD: &str = "connector:sip:password";

/// Feeds at most: a question reads them all at once.
const MAX_FEEDS: usize = 40;
const MAX_CONTACTS: usize = 50;

impl ConnectorId {
    pub const ALL: [ConnectorId; 6] = [
        ConnectorId::HomeAssistant,
        ConnectorId::Telegram,
        ConnectorId::LiveKit,
        ConnectorId::Sip,
        ConnectorId::Weather,
        ConnectorId::News,
    ];

    /// The id the connectors list and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectorId::HomeAssistant => "home-assistant",
            ConnectorId::Telegram => "telegram",
            ConnectorId::LiveKit => "livekit",
            ConnectorId::Sip => "sip",
            ConnectorId::Weather => "weather",
            ConnectorId::News => "news",
        }
    }

    pub fn parse(id: &str) -> Option<ConnectorId> {
        ConnectorId::ALL.into_iter().find(|known| known.as_str() == id)
    }

    pub fn title(self) -> &'static str {
        match self {
            ConnectorId::HomeAssistant => "Home Assistant",
            ConnectorId::Telegram => "Telegram",
            ConnectorId::LiveKit => "LiveKit",
            ConnectorId::Sip => "Телефония (SIP)",
            ConnectorId::Weather => "Погода",
            ConnectorId::News => "Новости",
        }
    }

    /// Part of the calls: only a PC's build has them.
    pub fn calls(self) -> bool {
        matches!(self, ConnectorId::Telegram | ConnectorId::LiveKit | ConnectorId::Sip)
    }

    /// Whether this build can use it. LiveKit's room needs its SDK — a prebuilt library the default build does
    /// not download — so without the feature it is not available either, «soon» in the panel rather than set up.
    /// SIP is frozen the same way until putting a station client on the column is a shipped flow: without it a
    /// call has no station to ring (live 09.11: the phone reaches the gateway, the gateway has no column).
    pub fn available(self) -> bool {
        if !self.calls() {
            return true;
        }
        if !cfg!(feature = "pc") {
            return false;
        }
        if self == ConnectorId::Sip {
            return false;
        }
        !matches!(self, ConnectorId::LiveKit) || cfg!(feature = "livekit")
    }

    fn key(self) -> &'static str {
        match self {
            ConnectorId::HomeAssistant => CONNECTOR_HOME_ASSISTANT.key,
            ConnectorId::Telegram => CONNECTOR_TELEGRAM.key,
            ConnectorId::LiveKit => CONNECTOR_LIVEKIT.key,
            ConnectorId::Sip => CONNECTOR_SIP.key,
            ConnectorId::Weather => CONNECTOR_WEATHER.key,
            ConnectorId::News => CONNECTOR_NEWS.key,
        }
    }

    /// Its secrets: the field the panel names, the secret's id.
    fn secrets(self) -> &'static [(&'static str, &'static str)] {
        match self {
            ConnectorId::HomeAssistant => &[("token", HA_TOKEN)],
            ConnectorId::Telegram => &[("token", TELEGRAM_TOKEN)],
            ConnectorId::LiveKit => &[("apiKey", LIVEKIT_KEY), ("apiSecret", LIVEKIT_SECRET)],
            ConnectorId::Sip => &[("password", SIP_PASSWORD)],
            ConnectorId::Weather | ConnectorId::News => &[],
        }
    }

    /// The env variables that seeded it.
    fn variables(self) -> &'static [&'static str] {
        match self {
            ConnectorId::HomeAssistant => &["HA_URL", "HA_TOKEN"],
            ConnectorId::Telegram => &["TELEGRAM_BOT_TOKEN", "TELEGRAM_CONTACTS", "ALERT_TELEGRAM_CHAT"],
            ConnectorId::LiveKit => &[
                "LIVEKIT_URL",
                "LIVEKIT_PUBLIC_URL",
                "LIVEKIT_PAGE_URL",
                "LIVEKIT_API_KEY",
                "LIVEKIT_API_SECRET",
                "LIVEKIT_ROOM",
            ],
            ConnectorId::Sip => {
                &["ARI_URL", "ARI_USER", "ARI_PASSWORD", "ARI_APP", "SIP_RTP_HOST", "SIP_RTP_PORT", "SIP_CONTACTS"]
            }
            ConnectorId::Weather => &["WEATHER_LOCATION"],
            ConnectorId::News => &["NEWS_FEEDS"],
        }
    }
}

/// The last «Проверить» of a connector: kept in memory, gone with a change or a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub ok: bool,
    pub text: String,
    pub at: String,
    /// Per part, when it has parts: a news feed by its address.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<CheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckItem {
    pub key: String,
    pub ok: bool,
    pub text: String,
}

impl CheckResult {
    pub fn new(ok: bool, text: impl Into<String>) -> CheckResult {
        CheckResult { ok, text: text.into(), at: now_iso(), items: Vec::new() }
    }
}

/// What the panel sees of a connector's settings: never a secret, contacts' targets by their last characters.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSettingsView {
    pub id: &'static str,
    pub title: &'static str,
    pub values: Value,
    pub secrets: BTreeMap<&'static str, SecretMask>,
    /// Stored: set up once, here or by the seed (false: never).
    pub stored: bool,
    /// Still the values .env seeded.
    pub from_env: bool,
    /// Its .env lines still there, not read any more.
    pub env_left: Vec<&'static str>,
    pub status: ConnectorStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckResult>,
    /// Part of this build (the calls are a PC's).
    pub available: bool,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    home: HomeAssistantSetting,
    telegram: TelegramSetting,
    livekit: LiveKitSetting,
    sip: SipSetting,
    weather: WeatherSetting,
    news: NewsSetting,
    /// Stored in the database: set up once, here or by the seed.
    stored: BTreeSet<ConnectorId>,
    /// Still as .env seeded them.
    seeded: BTreeSet<ConnectorId>,
}

type Listener = Arc<dyn Fn(ConnectorId) + Send + Sync>;

pub struct ConnectorSettings {
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    /// Where the bot keeps the voice messages it receives.
    telegram_inbox: PathBuf,
    state: RwLock<Snapshot>,
    /// The .env variables set at the start, by connector: the lines left to remove.
    env_left: BTreeMap<ConnectorId, Vec<&'static str>>,
    checks: Mutex<BTreeMap<ConnectorId, CheckResult>>,
    listeners: RwLock<Vec<Listener>>,
    /// One change at a time: a read-modify-write of a connector's row.
    mutation: Mutex<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A contact's target as the panel shows it: the last characters only (PRODUCT.md: no identifiers on screen).
pub fn contact_hint(to: &str) -> String {
    if to.chars().count() <= 4 {
        to.to_string()
    } else {
        let chars: Vec<char> = to.chars().collect();
        format!("…{}", chars[chars.len() - 4..].iter().collect::<String>())
    }
}

fn contacts_of(entries: &[ContactEntry]) -> Vec<(String, String)> {
    entries.iter().map(|entry| (entry.name.to_lowercase(), entry.to.clone())).collect()
}

fn entries_of(contacts: &[(String, String)]) -> Vec<ContactEntry> {
    contacts.iter().map(|(name, to)| ContactEntry { name: name.clone(), to: to.clone() }).collect()
}

fn filled(text: &str) -> Option<String> {
    Some(text.trim().to_string()).filter(|text| !text.is_empty())
}

impl ConnectorSettings {
    /// Seeds what the database has no row for from the environment, then reads every connector.
    pub fn new(
        seed: &ConnectorsSeed,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
        telegram_inbox: PathBuf,
    ) -> ConnectorSettings {
        let mut env_left: BTreeMap<ConnectorId, Vec<&'static str>> = BTreeMap::new();
        for name in &seed.set {
            if let Some(id) = ConnectorId::ALL.into_iter().find(|id| id.variables().contains(name)) {
                env_left.entry(id).or_default().push(name);
            }
        }
        let connectors = ConnectorSettings {
            settings,
            secrets,
            events,
            telegram_inbox,
            state: RwLock::new(Snapshot::default()),
            env_left,
            checks: Mutex::new(BTreeMap::new()),
            listeners: RwLock::new(Vec::new()),
            mutation: Mutex::new(()),
        };
        for id in ConnectorId::ALL {
            connectors.seed_one(id, seed);
        }
        connectors.reload();
        connectors
    }

    /// The connectors as the gateway keeps them, for a tool beside it (`duowave livekit-token`): the same database
    /// and key. A connector the database has no row for is seeded from .env first, as the gateway would.
    pub fn open(config: &crate::config::Config) -> anyhow::Result<ConnectorSettings> {
        let state_dir = config.skills.state_dir.clone();
        let db = Arc::new(crate::storage::Db::open(state_dir.as_deref())?);
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, state_dir.as_deref())?);
        Ok(ConnectorSettings::new(&config.connectors, settings, secrets, events, config.calls.telegram_inbox.clone()))
    }

    /// One connector from .env, when the database has no row for it.
    fn seed_one(&self, id: ConnectorId, seed: &ConnectorsSeed) {
        match self.settings.has(id.key()) {
            Ok(false) => {}
            Ok(true) => {
                if let Some(left) = self.env_left.get(&id) {
                    tracing::info!(
                        "connectors: {} in .env not read: {} is set up in the panel («Дом» → «Коннекторы»)",
                        left.join(", "),
                        id.title()
                    );
                }
                return;
            }
            Err(error) => {
                tracing::warn!("connectors: {} not seeded, its row not read: {error}", id.title());
                return;
            }
        }
        let seeded = match id {
            ConnectorId::HomeAssistant => seed.home_assistant.as_ref().map(|home| {
                let setting = HomeAssistantSetting { enabled: true, url: home.url.clone() };
                (self.settings.seed(&CONNECTOR_HOME_ASSISTANT, &setting), vec![(HA_TOKEN, home.token.clone())])
            }),
            ConnectorId::Telegram => seed.telegram.as_ref().map(|telegram| {
                let setting = TelegramSetting {
                    enabled: true,
                    contacts: entries_of(&telegram.contacts),
                    alert_chat: telegram.alert_chat.clone(),
                };
                (self.settings.seed(&CONNECTOR_TELEGRAM, &setting), vec![(TELEGRAM_TOKEN, telegram.token.clone())])
            }),
            ConnectorId::LiveKit => seed.livekit.as_ref().map(|livekit| {
                let setting = LiveKitSetting {
                    enabled: true,
                    url: livekit.url.clone(),
                    public_url: livekit.public_url.clone(),
                    page_url: livekit.page_url.clone(),
                    room: livekit.room.clone(),
                };
                (
                    self.settings.seed(&CONNECTOR_LIVEKIT, &setting),
                    vec![(LIVEKIT_KEY, livekit.api_key.clone()), (LIVEKIT_SECRET, livekit.api_secret.clone())],
                )
            }),
            ConnectorId::Sip => seed.sip.as_ref().map(|sip| {
                let setting = SipSetting {
                    enabled: true,
                    url: sip.url.clone(),
                    user: sip.user.clone(),
                    app: sip.app.clone(),
                    rtp_host: sip.rtp_host.clone(),
                    rtp_port: sip.rtp_port,
                    contacts: entries_of(&sip.contacts),
                };
                (self.settings.seed(&CONNECTOR_SIP, &setting), vec![(SIP_PASSWORD, sip.password.clone())])
            }),
            ConnectorId::Weather => seed.weather_location.as_ref().map(|location| {
                let setting = WeatherSetting { location: Some(location.clone()) };
                (self.settings.seed(&CONNECTOR_WEATHER, &setting), Vec::new())
            }),
            ConnectorId::News => (!seed.news_feeds.is_empty()).then(|| {
                let feeds = seed.news_feeds.iter().map(|url| feed_entry(url)).collect();
                (self.settings.seed(&CONNECTOR_NEWS, &NewsSetting { feeds }), Vec::new())
            }),
        };
        let Some((stored, secrets)) = seeded else { return };
        if let Err(error) = stored {
            tracing::warn!("connectors: {} not seeded from .env: {}", id.title(), error.message);
            return;
        }
        for (secret, value) in secrets {
            if let Err(error) = self.secrets.set(secret, Some(&value)) {
                tracing::warn!("connectors: {}'s secret not seeded: {}", id.title(), error.message);
            }
        }
        let names = self.env_left.get(&id).map(|names| names.join(", ")).unwrap_or_default();
        tracing::info!("connectors: {} moved in from .env ({names}); set up in the panel from now on", id.title());
    }

    /// Every connector from the database.
    fn reload(&self) {
        let mut snapshot = Snapshot::default();
        fn read<T: Default>(
            settings: &SettingsService,
            definition: &SettingDefinition<T>,
            id: ConnectorId,
            snapshot: &mut Snapshot,
        ) -> T {
            match settings.try_entry(definition) {
                Ok(Some(entry)) => {
                    snapshot.stored.insert(id);
                    if entry.is_default {
                        snapshot.seeded.insert(id);
                    }
                    entry.value
                }
                Ok(None) => {
                    if settings.has(definition.key).unwrap_or(false) {
                        tracing::warn!("connectors: {}'s stored row does not read; set it up again", id.title());
                        snapshot.stored.insert(id);
                    }
                    T::default()
                }
                Err(error) => {
                    tracing::warn!("connectors: {} not read: {error}", id.title());
                    T::default()
                }
            }
        }
        let settings = &self.settings;
        snapshot.home = read(settings, &CONNECTOR_HOME_ASSISTANT, ConnectorId::HomeAssistant, &mut snapshot);
        snapshot.telegram = read(settings, &CONNECTOR_TELEGRAM, ConnectorId::Telegram, &mut snapshot);
        snapshot.livekit = read(settings, &CONNECTOR_LIVEKIT, ConnectorId::LiveKit, &mut snapshot);
        snapshot.sip = read(settings, &CONNECTOR_SIP, ConnectorId::Sip, &mut snapshot);
        snapshot.weather = read(settings, &CONNECTOR_WEATHER, ConnectorId::Weather, &mut snapshot);
        snapshot.news = read(settings, &CONNECTOR_NEWS, ConnectorId::News, &mut snapshot);
        *self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }

    fn snapshot(&self) -> Snapshot {
        self.state.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Told after every change, outside the locks: the calls restart their bot and bridges, the Host check takes
    /// LiveKit's hosts.
    pub fn subscribe(&self, listener: impl Fn(ConnectorId) + Send + Sync + 'static) {
        self.listeners.write().unwrap_or_else(|poisoned| poisoned.into_inner()).push(Arc::new(listener));
    }

    fn changed(&self, id: ConnectorId) {
        let listeners = self.listeners.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        for listener in listeners {
            listener(id);
        }
    }

    // What the modules use: the values now, None when not set up, off or not complete.

    pub fn home_assistant(&self) -> Option<HomeAssistantConfig> {
        let home = self.snapshot().home;
        let url = filled(&home.url).filter(|_| home.enabled)?;
        let token = self.secrets.get(HA_TOKEN)?;
        Some(HomeAssistantConfig { url: url.trim_end_matches('/').to_string(), token })
    }

    pub fn telegram(&self) -> Option<TelegramConfig> {
        let telegram = self.snapshot().telegram;
        let token = self.secrets.get(TELEGRAM_TOKEN).filter(|_| telegram.enabled)?;
        Some(TelegramConfig {
            token,
            contacts: contacts_of(&telegram.contacts),
            inbox_dir: self.telegram_inbox.clone(),
            alert_chat: telegram.alert_chat,
        })
    }

    pub fn livekit(&self) -> Option<LiveKitConfig> {
        let livekit = self.snapshot().livekit;
        let url = filled(&livekit.url).filter(|_| livekit.enabled)?;
        Some(LiveKitConfig {
            url,
            public_url: livekit.public_url.as_deref().and_then(filled),
            page_url: livekit.page_url.as_deref().and_then(filled),
            api_key: self.secrets.get(LIVEKIT_KEY)?,
            api_secret: self.secrets.get(LIVEKIT_SECRET)?,
            room: filled(&livekit.room).unwrap_or_else(|| "home".into()),
        })
    }

    pub fn sip(&self) -> Option<SipConfig> {
        let sip = self.snapshot().sip;
        let url = filled(&sip.url).filter(|_| sip.enabled)?;
        Some(SipConfig {
            url,
            user: filled(&sip.user)?,
            password: self.secrets.get(SIP_PASSWORD)?,
            app: filled(&sip.app).unwrap_or_else(|| "duowave".into()),
            rtp_host: filled(&sip.rtp_host).unwrap_or_else(|| "127.0.0.1".into()),
            rtp_port: sip.rtp_port,
            contacts: contacts_of(&sip.contacts),
        })
    }

    /// The weather's usual place; None: the city of the station's time zone.
    pub fn weather_place(&self) -> Option<String> {
        self.snapshot().weather.location.as_deref().and_then(filled)
    }

    /// The news feeds switched on.
    pub fn news_feeds(&self) -> Vec<NewsFeedEntry> {
        self.snapshot().news.feeds.into_iter().filter(|feed| feed.enabled).collect()
    }

    /// Every feed in the list, the ones switched off too: «Проверить» looks at them all.
    pub fn all_news_feeds(&self) -> Vec<NewsFeedEntry> {
        self.snapshot().news.feeds
    }

    /// The hosts phones open LiveKit and its call page by: the Host check lets them in.
    pub fn livekit_hosts(&self) -> Vec<String> {
        let Some(livekit) = self.livekit() else { return Vec::new() };
        [livekit.public_url, livekit.page_url]
            .into_iter()
            .flatten()
            .filter_map(|url| crate::config::host_of_url(&url))
            .collect()
    }

    // The connectors list.

    /// Whether what it talks to is outside the house.
    pub fn cloud(&self, id: ConnectorId) -> bool {
        let snapshot = self.snapshot();
        match id {
            ConnectorId::HomeAssistant => !is_local_url(filled(&snapshot.home.url).as_deref()),
            ConnectorId::LiveKit => !is_local_url(filled(&snapshot.livekit.url).as_deref()),
            ConnectorId::Sip => false,
            ConnectorId::Telegram | ConnectorId::Weather | ConnectorId::News => true,
        }
    }

    pub fn status(&self, id: ConnectorId) -> ConnectorStatus {
        if !id.available() {
            // A computer's build without the piece says it is coming, so people don't look for a setting that
            // cannot work yet; the station's build simply has no calls.
            return if cfg!(feature = "pc") {
                ConnectorStatus::soon("пока недоступно — будет с обновлением")
            } else {
                ConnectorStatus::off("в сборке для колонки звонков нет")
            };
        }
        let snapshot = self.snapshot();
        let enabled = match id {
            ConnectorId::HomeAssistant => snapshot.home.enabled,
            ConnectorId::Telegram => snapshot.telegram.enabled,
            ConnectorId::LiveKit => snapshot.livekit.enabled,
            ConnectorId::Sip => snapshot.sip.enabled,
            ConnectorId::Weather | ConnectorId::News => true,
        };
        if !enabled {
            return ConnectorStatus::off("выключено");
        }
        if let Some(missing) = self.missing(id, &snapshot) {
            return ConnectorStatus::unset(missing);
        }
        match lock(&self.checks).get(&id) {
            Some(check) if check.ok => ConnectorStatus::ready(check.text.clone()),
            Some(check) => ConnectorStatus::error(check.text.clone()),
            None => ConnectorStatus::configured(Some(self.summary(id, &snapshot))),
        }
    }

    /// What it still needs, for people («нет токена», «нужны адрес и токен»); None: nothing.
    fn missing(&self, id: ConnectorId, snapshot: &Snapshot) -> Option<String> {
        let has = |secret: &str| self.secrets.has(secret);
        // Each part as «нужны …» and as «нет …» name it.
        let need = |parts: Vec<(&str, &str, bool)>| -> Option<String> {
            let lacking: Vec<(&str, &str)> =
                parts.iter().filter(|(_, _, ok)| !ok).map(|(named, none, _)| (*named, *none)).collect();
            match lacking.as_slice() {
                [] => None,
                [(_, none)] => Some(format!("нет {none}")),
                many => {
                    let names: Vec<&str> = many.iter().map(|(named, _)| *named).collect();
                    let (last, first) = names.split_last().expect("not empty");
                    Some(format!("нужны {} и {last}", first.join(", ")))
                }
            }
        };
        match id {
            ConnectorId::HomeAssistant => need(vec![
                ("адрес", "адреса", filled(&snapshot.home.url).is_some()),
                ("токен", "токена", has(HA_TOKEN)),
            ]),
            ConnectorId::Telegram => need(vec![("токен бота", "токена бота", has(TELEGRAM_TOKEN))]),
            ConnectorId::LiveKit => need(vec![
                ("адрес", "адреса", filled(&snapshot.livekit.url).is_some()),
                ("ключ", "ключа", has(LIVEKIT_KEY)),
                ("секрет", "секрета", has(LIVEKIT_SECRET)),
            ]),
            ConnectorId::Sip => need(vec![
                ("адрес ARI", "адреса ARI", filled(&snapshot.sip.url).is_some()),
                ("логин", "логина", filled(&snapshot.sip.user).is_some()),
                ("пароль", "пароля", has(SIP_PASSWORD)),
            ]),
            ConnectorId::Weather => None,
            ConnectorId::News => (!snapshot.news.feeds.iter().any(|feed| feed.enabled)).then(|| "лент нет".to_string()),
        }
    }

    /// What it uses, for people: a host, how many contacts or feeds.
    fn summary(&self, id: ConnectorId, snapshot: &Snapshot) -> String {
        let contacts = |count: usize| {
            if count == 0 {
                "контактов нет".to_string()
            } else {
                format!("контактов: {count}")
            }
        };
        match id {
            ConnectorId::HomeAssistant => host_of(&snapshot.home.url),
            ConnectorId::Telegram => contacts(snapshot.telegram.contacts.len()),
            ConnectorId::LiveKit => host_of(
                snapshot.livekit.public_url.as_deref().and_then(filled).as_deref().unwrap_or(&snapshot.livekit.url),
            ),
            ConnectorId::Sip => format!("{} · {}", host_of(&snapshot.sip.url), contacts(snapshot.sip.contacts.len())),
            ConnectorId::Weather => match snapshot.weather.location.as_deref().and_then(filled) {
                Some(place) => place,
                None => "город по часовому поясу".into(),
            },
            ConnectorId::News => {
                let on = snapshot.news.feeds.iter().filter(|feed| feed.enabled).count();
                format!("лент: {on}")
            }
        }
    }

    pub fn record_check(&self, id: ConnectorId, check: CheckResult) {
        lock(&self.checks).insert(id, check);
    }

    pub fn last_check(&self, id: ConnectorId) -> Option<CheckResult> {
        lock(&self.checks).get(&id).cloned()
    }

    // The panel.

    pub fn view(&self, id: ConnectorId) -> ConnectorSettingsView {
        let snapshot = self.snapshot();
        let contacts = |entries: &[ContactEntry]| -> Vec<Value> {
            entries.iter().map(|entry| json!({ "name": entry.name, "hint": contact_hint(&entry.to) })).collect()
        };
        let values = match id {
            ConnectorId::HomeAssistant => json!({ "enabled": snapshot.home.enabled, "url": snapshot.home.url }),
            ConnectorId::Telegram => {
                let telegram = &snapshot.telegram;
                let alert = telegram.alert_chat.as_deref();
                let alert_to = alert.and_then(|chat| telegram.contacts.iter().find(|entry| entry.to == chat));
                json!({
                    "enabled": telegram.enabled,
                    "contacts": contacts(&telegram.contacts),
                    "alertTo": alert_to.map(|entry| entry.name.clone()),
                    "alertHint": alert.filter(|_| alert_to.is_none()).map(contact_hint),
                })
            }
            ConnectorId::LiveKit => json!({
                "enabled": snapshot.livekit.enabled,
                "url": snapshot.livekit.url,
                "publicUrl": snapshot.livekit.public_url,
                "pageUrl": snapshot.livekit.page_url,
                "room": snapshot.livekit.room,
            }),
            ConnectorId::Sip => json!({
                "enabled": snapshot.sip.enabled,
                "url": snapshot.sip.url,
                "user": snapshot.sip.user,
                "app": snapshot.sip.app,
                "rtpHost": snapshot.sip.rtp_host,
                "rtpPort": snapshot.sip.rtp_port,
                "contacts": contacts(&snapshot.sip.contacts),
            }),
            ConnectorId::Weather => json!({ "location": snapshot.weather.location }),
            ConnectorId::News => json!({ "feeds": snapshot.news.feeds }),
        };
        ConnectorSettingsView {
            id: id.as_str(),
            title: id.title(),
            values,
            secrets: id.secrets().iter().map(|(field, secret)| (*field, self.secrets.mask(secret))).collect(),
            stored: snapshot.stored.contains(&id),
            from_env: snapshot.seeded.contains(&id),
            env_left: self.env_left.get(&id).cloned().unwrap_or_default(),
            status: self.status(id),
            check: self.last_check(id),
            available: id.available(),
        }
    }

    /// The panel's change: only the fields given; a secret given empty is removed. Applied at once; any change, a
    /// secret's alone included, makes the row the panel's (no longer .env's).
    pub fn update(
        &self,
        id: ConnectorId,
        input: &Map<String, Value>,
        actor: Option<Actor>,
    ) -> Result<ConnectorSettingsView, ApiError> {
        if !id.available() {
            return Err(ApiError::bad_request(if cfg!(feature = "pc") {
                "Пока недоступно — будет с обновлением."
            } else {
                "В сборке для колонки звонков нет."
            }));
        }
        let guard = lock(&self.mutation);
        let snapshot = self.snapshot();
        let mut fields: Vec<String> = Vec::new();
        let secrets: Vec<(&'static str, &'static str)> = id.secrets().to_vec();
        let mut input = input.clone();
        // Secrets first out of the input: what stays is the plain fields.
        let mut secret_changes: Vec<(&'static str, Option<String>)> = Vec::new();
        for (field, secret) in &secrets {
            if let Some(value) = input.remove(*field) {
                let Value::String(value) = value else {
                    return Err(ApiError::bad_request(format!("{field}: нужна строка.")));
                };
                let value = value.trim().to_string();
                if value.chars().any(char::is_whitespace) {
                    return Err(ApiError::bad_request("В ключе не бывает пробелов: вставьте его ещё раз."));
                }
                secret_changes.push((secret, Some(value).filter(|value| !value.is_empty())));
                fields.push(field.to_string());
            }
        }
        let input = Fields(input);
        match id {
            ConnectorId::HomeAssistant => {
                let mut home = snapshot.home.clone();
                input.only(&["enabled", "url"])?;
                if let Some(enabled) = input.flag("enabled")? {
                    home.enabled = enabled;
                }
                if let Some(url) = input.url("url", "Адрес Home Assistant", &["http", "https"])? {
                    home.url = url;
                }
                fields.extend(input.changed(&json!(snapshot.home), &json!(home)));
                self.settings.set(&CONNECTOR_HOME_ASSISTANT, &home, actor.clone())?;
            }
            ConnectorId::Telegram => {
                let mut telegram = snapshot.telegram.clone();
                input.only(&["enabled", "contacts", "alertTo"])?;
                if let Some(enabled) = input.flag("enabled")? {
                    telegram.enabled = enabled;
                }
                if let Some(contacts) = input.contacts("contacts", &telegram.contacts, ContactKind::Chat)? {
                    telegram.contacts = contacts;
                }
                if let Some(alert) = input.text("alertTo", 60)? {
                    telegram.alert_chat = if alert.is_empty() {
                        None
                    } else {
                        let found =
                            telegram.contacts.iter().find(|entry| entry.name.to_lowercase() == alert.to_lowercase());
                        match found {
                            Some(entry) => Some(entry.to.clone()),
                            None => return Err(ApiError::bad_request(format!("Оповещения: нет контакта «{alert}»."))),
                        }
                    };
                }
                fields.extend(input.changed(&json!(snapshot.telegram), &json!(telegram)));
                self.settings.set(&CONNECTOR_TELEGRAM, &telegram, actor.clone())?;
            }
            ConnectorId::LiveKit => {
                let mut livekit = snapshot.livekit.clone();
                input.only(&["enabled", "url", "publicUrl", "pageUrl", "room"])?;
                if let Some(enabled) = input.flag("enabled")? {
                    livekit.enabled = enabled;
                }
                if let Some(url) = input.url("url", "Адрес LiveKit для шлюза", &["ws", "wss", "http", "https"])?
                {
                    livekit.url = url;
                }
                if let Some(url) = input.url("publicUrl", "Адрес LiveKit снаружи", &["ws", "wss"])? {
                    livekit.public_url = Some(url).filter(|url| !url.is_empty());
                }
                if let Some(url) = input.url("pageUrl", "Адрес страницы звонка", &["http", "https"])?
                {
                    livekit.page_url = Some(url).filter(|url| !url.is_empty());
                }
                if let Some(room) = input.text("room", 64)? {
                    if !room.is_empty() && !room.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                        return Err(ApiError::bad_request("Комната: латинские буквы, цифры, - и _."));
                    }
                    livekit.room = if room.is_empty() { "home".into() } else { room };
                }
                fields.extend(input.changed(&json!(snapshot.livekit), &json!(livekit)));
                self.settings.set(&CONNECTOR_LIVEKIT, &livekit, actor.clone())?;
            }
            ConnectorId::Sip => {
                let mut sip = snapshot.sip.clone();
                input.only(&["enabled", "url", "user", "app", "rtpHost", "rtpPort", "contacts"])?;
                if let Some(enabled) = input.flag("enabled")? {
                    sip.enabled = enabled;
                }
                if let Some(url) = input.url("url", "Адрес ARI", &["http", "https"])? {
                    sip.url = url;
                }
                if let Some(user) = input.text("user", 64)? {
                    sip.user = user;
                }
                if let Some(app) = input.text("app", 64)? {
                    if !app.is_empty() && !app.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                        return Err(ApiError::bad_request("Приложение ARI: латинские буквы, цифры, - и _."));
                    }
                    sip.app = if app.is_empty() { "duowave".into() } else { app };
                }
                if let Some(host) = input.text("rtpHost", 100)? {
                    sip.rtp_host = if host.is_empty() { "127.0.0.1".into() } else { host };
                }
                if let Some(port) = input.port("rtpPort")? {
                    sip.rtp_port = port;
                }
                if let Some(contacts) = input.contacts("contacts", &sip.contacts, ContactKind::Phone)? {
                    sip.contacts = contacts;
                }
                fields.extend(input.changed(&json!(snapshot.sip), &json!(sip)));
                self.settings.set(&CONNECTOR_SIP, &sip, actor.clone())?;
            }
            ConnectorId::Weather => {
                input.only(&["location"])?;
                let mut weather = snapshot.weather.clone();
                if let Some(location) = input.text("location", 100)? {
                    weather.location = Some(location).filter(|location| !location.is_empty());
                }
                fields.extend(input.changed(&json!(snapshot.weather), &json!(weather)));
                self.settings.set(&CONNECTOR_WEATHER, &weather, actor.clone())?;
            }
            ConnectorId::News => {
                input.only(&["feeds"])?;
                let mut news = snapshot.news.clone();
                if let Some(feeds) = input.feeds("feeds")? {
                    news.feeds = feeds;
                }
                fields.extend(input.changed(&json!(snapshot.news), &json!(news)));
                self.settings.set(&CONNECTOR_NEWS, &news, actor.clone())?;
            }
        }
        for (secret, value) in &secret_changes {
            self.secrets.set(secret, value.as_deref())?;
        }
        self.reload();
        lock(&self.checks).remove(&id);
        drop(guard);
        self.events.emit(GatewayEvent::ConnectorChanged { connector: id.as_str().into(), fields, actor });
        self.changed(id);
        Ok(self.view(id))
    }

    /// Clears a connector: «не настроено». Its row stays, empty, so .env does not seed it again.
    pub fn clear(&self, id: ConnectorId, actor: Option<Actor>) -> Result<ConnectorSettingsView, ApiError> {
        let guard = lock(&self.mutation);
        match id {
            ConnectorId::HomeAssistant => {
                self.settings.set(&CONNECTOR_HOME_ASSISTANT, &HomeAssistantSetting::default(), actor.clone())?
            }
            ConnectorId::Telegram => {
                self.settings.set(&CONNECTOR_TELEGRAM, &TelegramSetting::default(), actor.clone())?
            }
            ConnectorId::LiveKit => self.settings.set(&CONNECTOR_LIVEKIT, &LiveKitSetting::default(), actor.clone())?,
            ConnectorId::Sip => self.settings.set(&CONNECTOR_SIP, &SipSetting::default(), actor.clone())?,
            ConnectorId::Weather => self.settings.set(&CONNECTOR_WEATHER, &WeatherSetting::default(), actor.clone())?,
            ConnectorId::News => self.settings.set(&CONNECTOR_NEWS, &NewsSetting::default(), actor.clone())?,
        }
        for (_, secret) in id.secrets() {
            self.secrets.delete(secret)?;
        }
        self.reload();
        lock(&self.checks).remove(&id);
        drop(guard);
        self.events.emit(GatewayEvent::ConnectorChanged {
            connector: id.as_str().into(),
            fields: vec!["cleared".into()],
            actor,
        });
        self.changed(id);
        Ok(self.view(id))
    }
}

/// A feed from .env or an address alone: named and topiced as the catalog knows it.
fn feed_entry(url: &str) -> NewsFeedEntry {
    let known = NEWS_CATALOG.feed_by_url(url);
    NewsFeedEntry {
        url: url.trim().to_string(),
        title: known.map(|feed| feed.title.clone()),
        topic: known.map(|feed| feed.topic.clone()),
        catalog: known.map(|feed| feed.id.clone()),
        enabled: true,
    }
}

#[derive(Clone, Copy)]
enum ContactKind {
    /// A Telegram chat: its number (a group's starts with -) or a channel's @name.
    Chat,
    /// A phone number or an extension.
    Phone,
}

/// The panel's input of a connector, read field by field with its own messages.
struct Fields(Map<String, Value>);

impl Fields {
    /// Only these fields: anything else is the panel's mistake, said plainly.
    fn only(&self, known: &[&str]) -> Result<(), ApiError> {
        match self.0.keys().find(|key| !known.contains(&key.as_str())) {
            Some(key) => Err(ApiError::bad_request(format!("Неизвестное поле: {key}."))),
            None => Ok(()),
        }
    }

    fn flag(&self, name: &str) -> Result<Option<bool>, ApiError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(ApiError::bad_request(format!("{name}: нужно да или нет."))),
        }
    }

    /// A text, trimmed; Some("") clears it.
    fn text(&self, name: &str, max: usize) -> Result<Option<String>, ApiError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(String::new())),
            Some(Value::String(text)) => {
                let text = text.trim();
                if text.chars().count() > max {
                    return Err(ApiError::bad_request(format!("{name}: не длиннее {max} знаков.")));
                }
                Ok(Some(text.to_string()))
            }
            Some(_) => Err(ApiError::bad_request(format!("{name}: нужна строка."))),
        }
    }

    /// An address with one of these schemes; Some("") clears it.
    fn url(&self, name: &str, title: &str, schemes: &[&str]) -> Result<Option<String>, ApiError> {
        let Some(text) = self.text(name, 300)? else { return Ok(None) };
        if text.is_empty() {
            return Ok(Some(text));
        }
        let parsed = url::Url::parse(&text).ok().filter(|url| schemes.contains(&url.scheme()) && url.host().is_some());
        match parsed {
            Some(_) => Ok(Some(text.trim_end_matches('/').to_string())),
            None => Err(ApiError::bad_request(format!("{title}: нужен адрес вида {}://…", schemes.join("://… или ")))),
        }
    }

    fn port(&self, name: &str) -> Result<Option<u16>, ApiError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(value) => match value.as_u64().filter(|port| (1..=65_535).contains(port)) {
                Some(port) => Ok(Some(port as u16)),
                None => Err(ApiError::bad_request("Порт: число от 1 до 65535.")),
            },
        }
    }

    /// The contacts, the whole list: `{name, to}` sets a target, `{name, was}` keeps the target of the contact that
    /// was named `was` (the panel sees targets by their last characters only), `{name}` keeps its own.
    fn contacts(
        &self,
        name: &str,
        existing: &[ContactEntry],
        kind: ContactKind,
    ) -> Result<Option<Vec<ContactEntry>>, ApiError> {
        let Some(value) = self.0.get(name) else { return Ok(None) };
        let Some(items) = value.as_array() else {
            return Err(ApiError::bad_request("Контакты: нужен список."));
        };
        if items.len() > MAX_CONTACTS {
            return Err(ApiError::bad_request(format!("Контактов не больше {MAX_CONTACTS}.")));
        }
        let mut result: Vec<ContactEntry> = Vec::new();
        for item in items {
            let text = |field: &str| item.get(field).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
            let Some(contact) = text("name") else {
                return Err(ApiError::bad_request("У контакта нет имени."));
            };
            if contact.chars().count() > 40 {
                return Err(ApiError::bad_request(format!("Имя «{contact}»: не длиннее 40 знаков.")));
            }
            if result.iter().any(|known| known.name.to_lowercase() == contact.to_lowercase()) {
                return Err(ApiError::bad_request(format!("Имя «{contact}» встречается дважды.")));
            }
            let kept = |was: &str| {
                existing
                    .iter()
                    .find(|known| known.name.to_lowercase() == was.to_lowercase())
                    .map(|known| known.to.clone())
            };
            let to = match text("to") {
                Some(to) => to.to_string(),
                None => match kept(text("was").unwrap_or(contact)) {
                    Some(to) => to,
                    None => {
                        return Err(ApiError::bad_request(match kind {
                            ContactKind::Chat => format!("У контакта «{contact}» нет номера чата."),
                            ContactKind::Phone => format!("У контакта «{contact}» нет номера."),
                        }))
                    }
                },
            };
            let fits = match kind {
                ContactKind::Chat => {
                    let digits = to.strip_prefix('-').unwrap_or(&to);
                    (!digits.is_empty() && digits.len() <= 20 && digits.chars().all(|c| c.is_ascii_digit()))
                        || to.strip_prefix('@').is_some_and(|handle| {
                            (4..=32).contains(&handle.len())
                                && handle.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        })
                }
                ContactKind::Phone => {
                    let digits = to.strip_prefix('+').unwrap_or(&to);
                    !digits.is_empty()
                        && digits.len() <= 20
                        && digits.chars().all(|c| c.is_ascii_digit() || c == '*' || c == '#')
                }
            };
            if !fits {
                return Err(ApiError::bad_request(match kind {
                    ContactKind::Chat => format!("Чат «{contact}»: нужен номер чата (цифры) или @имя канала."),
                    ContactKind::Phone => format!("Номер «{contact}»: цифры, можно с + в начале."),
                }));
            }
            result.push(ContactEntry { name: contact.to_string(), to });
        }
        Ok(Some(result))
    }

    /// The feeds, the whole list: an address each, named and topiced from the catalog when it is there.
    fn feeds(&self, name: &str) -> Result<Option<Vec<NewsFeedEntry>>, ApiError> {
        let Some(value) = self.0.get(name) else { return Ok(None) };
        let Some(items) = value.as_array() else {
            return Err(ApiError::bad_request("Ленты: нужен список."));
        };
        if items.len() > MAX_FEEDS {
            return Err(ApiError::bad_request(format!("Лент не больше {MAX_FEEDS}: вопрос открывает их все сразу.")));
        }
        let mut result: Vec<NewsFeedEntry> = Vec::new();
        for item in items {
            let text = |field: &str| item.get(field).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
            let Some(url) = text("url") else {
                return Err(ApiError::bad_request("У ленты нет адреса."));
            };
            let parsed = url::Url::parse(url).ok().filter(|parsed| matches!(parsed.scheme(), "http" | "https"));
            if parsed.is_none() || url.len() > 500 {
                return Err(ApiError::bad_request(format!("Лента «{url}»: нужен адрес вида https://…")));
            }
            if result.iter().any(|known| same_feed(&known.url, url)) {
                continue;
            }
            let mut entry = feed_entry(url);
            if let Some(title) = text("title") {
                entry.title = Some(title.chars().take(120).collect());
            }
            if let Some(topic) = text("topic") {
                if NEWS_CATALOG.topic(topic).is_none() {
                    return Err(ApiError::bad_request(format!("Неизвестная тема: {topic}.")));
                }
                entry.topic = Some(topic.to_string());
            }
            if let Some(enabled) = item.get("enabled").and_then(Value::as_bool) {
                entry.enabled = enabled;
            }
            result.push(entry);
        }
        Ok(Some(result))
    }

    /// The fields given whose value changed.
    fn changed(&self, before: &Value, after: &Value) -> Vec<String> {
        self.0.keys().filter(|key| before.get(key.as_str()) != after.get(key.as_str())).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::connectors::ConnectorState;
    use crate::storage::Db;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Parts {
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
        dir: tempfile::TempDir,
    }

    fn parts() -> Parts {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(dir.path())).unwrap());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db.clone(), Some(dir.path())).unwrap());
        Parts { db, settings, secrets, events, dir }
    }

    fn seed(pairs: &[(&str, &str)]) -> ConnectorsSeed {
        Config::for_tests(&Env::from_pairs(pairs.iter().copied())).connectors
    }

    fn build(parts: &Parts, pairs: &[(&str, &str)]) -> ConnectorSettings {
        ConnectorSettings::new(
            &seed(pairs),
            parts.settings.clone(),
            parts.secrets.clone(),
            parts.events.clone(),
            parts.dir.path().join("messages"),
        )
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    const ENV: &[(&str, &str)] = &[
        ("TELEGRAM_BOT_TOKEN", "123456:telegram-token-abcdef"),
        ("TELEGRAM_CONTACTS", "Мама=111222333;папа=444555666"),
        ("ALERT_TELEGRAM_CHAT", "444555666"),
        ("LIVEKIT_URL", "ws://127.0.0.1:7880"),
        ("LIVEKIT_PUBLIC_URL", "wss://lk.example.com"),
        ("LIVEKIT_PAGE_URL", "https://call.example.com/calls/livekit"),
        ("LIVEKIT_API_KEY", "APIkey123456"),
        ("LIVEKIT_API_SECRET", "livekit-secret-123456"),
        ("NEWS_FEEDS", "https://tass.ru/rss/v2.xml,https://example.com/feed"),
    ];

    #[test]
    fn moves_the_env_in_once_and_reads_it_no_more() {
        let parts = parts();
        let first = build(&parts, ENV);
        let telegram = first.telegram().unwrap();
        assert_eq!(telegram.token, "123456:telegram-token-abcdef");
        assert_eq!(telegram.contacts, vec![("мама".into(), "111222333".into()), ("папа".into(), "444555666".into())]);
        assert_eq!(telegram.alert_chat.as_deref(), Some("444555666"));
        let livekit = first.livekit().unwrap();
        assert_eq!((livekit.room.as_str(), livekit.api_key.as_str()), ("home", "APIkey123456"));
        assert_eq!(first.livekit_hosts(), vec!["lk.example.com", "call.example.com"]);
        let feeds = first.news_feeds();
        assert_eq!(feeds[0].catalog.as_deref(), Some("tass"));
        assert_eq!(feeds[0].topic.as_deref(), Some("main"));
        assert_eq!(feeds[1].catalog, None);
        assert!(first.home_assistant().is_none() && first.sip().is_none());
        let view = first.view(ConnectorId::Telegram);
        assert!(view.stored && view.from_env);
        assert_eq!(view.env_left, vec!["TELEGRAM_BOT_TOKEN", "TELEGRAM_CONTACTS", "ALERT_TELEGRAM_CHAT"]);
        assert_eq!(view.values["alertTo"], json!("папа"));
        assert_eq!(view.values["contacts"][0], json!({"name": "мама", "hint": "…2333"}));
        let printed = serde_json::to_string(&view).unwrap();
        assert!(!printed.contains("telegram-token") && !printed.contains("111222333"));
        let home = first.view(ConnectorId::HomeAssistant);
        assert!(!home.stored);
        assert_eq!(home.status, ConnectorStatus::unset("нужны адрес и токен"));

        // The panel changes the bot's token; a later start with other .env values keeps the panel's.
        first.update(ConnectorId::Telegram, &object(json!({"token": "999:other-token-abcdef"})), None).unwrap();
        let again = build(
            &parts,
            &[
                ("TELEGRAM_BOT_TOKEN", "1:env-again"),
                ("HA_URL", "http://ha.local:8123"),
                ("HA_TOKEN", "ha-token-123456"),
            ],
        );
        assert_eq!(again.telegram().unwrap().token, "999:other-token-abcdef");
        assert!(!again.view(ConnectorId::Telegram).from_env, "a secret's change makes the row the panel's");
        // Home Assistant had no row: its .env values move in now.
        assert_eq!(again.home_assistant().unwrap().url, "http://ha.local:8123");
        assert_eq!(again.livekit().unwrap().api_secret, "livekit-secret-123456");
    }

    #[test]
    fn a_cleared_connector_stays_cleared_whatever_env_says() {
        let parts = parts();
        let connectors = build(&parts, ENV);
        connectors.clear(ConnectorId::LiveKit, None).unwrap();
        assert!(connectors.livekit().is_none());
        assert!(connectors.livekit_hosts().is_empty());
        assert!(!parts.secrets.has(LIVEKIT_SECRET));
        let again = build(&parts, ENV);
        assert!(again.livekit().is_none(), "the empty row is not seeded again");
        if cfg!(feature = "livekit") {
            assert_eq!(again.status(ConnectorId::LiveKit), ConnectorStatus::unset("нужны адрес, ключ и секрет"));
        }
        let stored: String = parts
            .db
            .with(|c| c.query_row("SELECT ciphertext FROM secrets WHERE id = ?1", [TELEGRAM_TOKEN], |row| row.get(0)))
            .unwrap();
        assert!(!stored.contains("telegram-token"), "the seeded token is sealed");
    }

    #[test]
    fn applies_a_change_at_once_and_tells_its_listeners_and_the_audit() {
        let parts = parts();
        let connectors = build(&parts, &[]);
        let heard = Arc::new(AtomicUsize::new(0));
        let counted = heard.clone();
        connectors.subscribe(move |id| {
            assert_eq!(id, ConnectorId::HomeAssistant);
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let mut events = parts.events.subscribe();
        let view = connectors
            .update(
                ConnectorId::HomeAssistant,
                &object(json!({"url": "http://192.168.1.5:8123/", "token": "eyJ-ha-token-123456"})),
                None,
            )
            .unwrap();
        assert_eq!(heard.load(Ordering::SeqCst), 1);
        assert_eq!(connectors.home_assistant().unwrap().url, "http://192.168.1.5:8123");
        assert_eq!(view.status, ConnectorStatus::configured(Some("192.168.1.5:8123".into())));
        assert_eq!(view.secrets["token"].hint.as_deref(), Some("…3456"));
        assert!(!connectors.cloud(ConnectorId::HomeAssistant));
        let mut payloads = Vec::new();
        while let Ok(event) = events.try_recv() {
            payloads.push((event.name(), event.payload()));
        }
        let names: Vec<&str> = payloads.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["settings.changed", "connector.changed"]);
        let changed = &payloads[1].1;
        assert_eq!(changed["connector"], json!("home-assistant"));
        assert_eq!(changed["fields"], json!(["token", "url"]));
        assert!(!changed.to_string().contains("eyJ-ha-token"));

        connectors.update(ConnectorId::HomeAssistant, &object(json!({"enabled": false})), None).unwrap();
        assert!(connectors.home_assistant().is_none());
        assert_eq!(connectors.status(ConnectorId::HomeAssistant), ConnectorStatus::off("выключено"));
        connectors.record_check(ConnectorId::HomeAssistant, CheckResult::new(true, "Home Assistant 2026.10"));
        connectors.update(ConnectorId::HomeAssistant, &object(json!({"enabled": true})), None).unwrap();
        assert_eq!(
            connectors.status(ConnectorId::HomeAssistant).state,
            ConnectorState::Configured,
            "a change forgets the last check"
        );
        connectors.record_check(ConnectorId::HomeAssistant, CheckResult::new(false, "токен не подходит"));
        assert_eq!(connectors.status(ConnectorId::HomeAssistant), ConnectorStatus::error("токен не подходит"));
    }

    #[test]
    fn keeps_contacts_targets_the_panel_sees_only_by_their_ends() {
        let parts = parts();
        let connectors = build(&parts, ENV);
        // «мама» renamed and kept, «папа» kept, a new one with its chat, the alert moved to the new one.
        connectors
            .update(
                ConnectorId::Telegram,
                &object(json!({"contacts": [
                    {"name": "Мамочка", "was": "мама"},
                    {"name": "папа"},
                    {"name": "бабушка", "to": "-100777"},
                ], "alertTo": "бабушка"})),
                None,
            )
            .unwrap();
        let telegram = connectors.telegram().unwrap();
        assert_eq!(
            telegram.contacts,
            vec![
                ("мамочка".into(), "111222333".into()),
                ("папа".into(), "444555666".into()),
                ("бабушка".into(), "-100777".into())
            ]
        );
        assert_eq!(telegram.alert_chat.as_deref(), Some("-100777"));
        let problem =
            |input: Value| connectors.update(ConnectorId::Telegram, &object(input), None).unwrap_err().message;
        assert_eq!(problem(json!({"contacts": [{"name": "дядя"}]})), "У контакта «дядя» нет номера чата.");
        assert_eq!(
            problem(json!({"contacts": [{"name": "а", "to": "1"}, {"name": "А", "to": "2"}]})),
            "Имя «А» встречается дважды."
        );
        assert_eq!(
            problem(json!({"contacts": [{"name": "а", "to": "12 34"}]})),
            "Чат «а»: нужен номер чата (цифры) или @имя канала."
        );
        assert_eq!(problem(json!({"alertTo": "сосед"})), "Оповещения: нет контакта «сосед».");
        assert_eq!(problem(json!({"what": 1})), "Неизвестное поле: what.");
        // Sip is frozen («скоро», live 09.11: a call has no station to ring without a station client on the
        // column): its settings stay closed, and even a full set of values is refused.
        assert!(!ConnectorId::Sip.available());
        assert_eq!(
            connectors
                .update(
                    ConnectorId::Sip,
                    &object(json!({"url": "http://127.0.0.1:8088/ari", "user": "duowave", "password": "ari-pass-123",
                "contacts": [{"name": "мама", "to": "+79990001122"}]})),
                    None
                )
                .unwrap_err()
                .message,
            "Пока недоступно — будет с обновлением."
        );
    }

    #[test]
    fn says_what_an_address_must_be() {
        let parts = parts();
        let connectors = build(&parts, &[]);
        let problem = |id, input: Value| connectors.update(id, &object(input), None).unwrap_err().message;
        assert_eq!(
            problem(ConnectorId::HomeAssistant, json!({"url": "ha.local:8123"})),
            "Адрес Home Assistant: нужен адрес вида http://… или https://…"
        );
        assert_eq!(
            problem(ConnectorId::LiveKit, json!({"publicUrl": "https://lk.example.com"})),
            if cfg!(feature = "livekit") {
                "Адрес LiveKit снаружи: нужен адрес вида ws://… или wss://…".to_string()
            } else {
                "Пока недоступно — будет с обновлением.".to_string()
            }
        );
        assert_eq!(
            problem(ConnectorId::Telegram, json!({"token": "a b"})),
            "В ключе не бывает пробелов: вставьте его ещё раз."
        );
        assert_eq!(
            problem(ConnectorId::News, json!({"feeds": [{"url": "ftp://x/y"}]})),
            "Лента «ftp://x/y»: нужен адрес вида https://…"
        );
        let news = connectors
            .update(
                ConnectorId::News,
                &object(json!({"feeds": [
                    {"url": "https://lenta.ru/rss/"},
                    {"url": "https://lenta.ru/rss"},
                    {"url": "https://example.com/rss", "title": "Свой блог", "topic": "tech", "enabled": false},
                ]})),
                None,
            )
            .unwrap();
        assert_eq!(news.values["feeds"].as_array().unwrap().len(), 2, "the same feed once");
        assert_eq!(news.values["feeds"][0]["catalog"], json!("lenta"));
        assert_eq!(connectors.news_feeds().len(), 1, "a feed switched off is not read");
        assert_eq!(connectors.status(ConnectorId::News), ConnectorStatus::configured(Some("лент: 1".into())));
        assert_eq!(
            problem(ConnectorId::News, json!({"feeds": [{"url": "https://a.ru/rss", "topic": "cats"}]})),
            "Неизвестная тема: cats."
        );
    }

    #[test]
    fn weather_follows_the_time_zone_until_a_place_is_given() {
        let parts = parts();
        let connectors = build(&parts, &[]);
        assert_eq!(connectors.weather_place(), None);
        assert_eq!(
            connectors.status(ConnectorId::Weather),
            ConnectorStatus::configured(Some("город по часовому поясу".into()))
        );
        connectors.update(ConnectorId::Weather, &object(json!({"location": " Новосибирск "})), None).unwrap();
        assert_eq!(connectors.weather_place().as_deref(), Some("Новосибирск"));
        connectors.update(ConnectorId::Weather, &object(json!({"location": ""})), None).unwrap();
        assert_eq!(connectors.weather_place(), None);
    }

    #[test]
    fn hides_contacts_targets_but_their_ends() {
        assert_eq!(contact_hint("111222333"), "…2333");
        assert_eq!(contact_hint("101"), "101");
    }

    #[test]
    fn livekit_and_sip_are_soon_not_unset() {
        // LiveKit's SDK is not in the default build, and Sip is frozen until a station client on the column is a
        // shipped flow: the panel says both are coming instead of asking for what could not work.
        if !cfg!(feature = "livekit") {
            let connectors = build(&parts(), &[]);
            let status = connectors.status(ConnectorId::LiveKit);
            assert_eq!(status.state, ConnectorState::Soon);
            assert!(status.detail.is_some_and(|detail| detail.contains("недоступно")));
            assert!(!ConnectorId::LiveKit.available());
        }
        let connectors = build(&parts(), &[]);
        assert_eq!(connectors.status(ConnectorId::Sip).state, ConnectorState::Soon);
        assert!(!ConnectorId::Sip.available());
        // What this build does have stays an ordinary unset: asking for its missing token is honest.
        assert_eq!(connectors.status(ConnectorId::Telegram).state, ConnectorState::Unset);
    }
}
