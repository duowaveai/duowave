//! Every setting the gateway stores, in one place. The keys are the ones the Node gateway wrote. A new setting is a
//! definition here, read and written through the SettingsService — never a string key in a module.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::identity::principal::Restrictions;
use crate::core::identity::roles::{normalize_role, Role};
use crate::core::settings::{
    as_object, as_one_of, as_string, as_string_list, as_string_record, SettingDefinition, SettingScope,
};

/// The panel's mode: Яндекс (as before the gateway), Шлюз, Свой (the gateway without Yandex).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelMode {
    Relay,
    Gateway,
    Own,
}

impl PanelMode {
    pub fn parse(value: &str) -> Option<PanelMode> {
        match value {
            "relay" => Some(PanelMode::Relay),
            "gateway" => Some(PanelMode::Gateway),
            "own" => Some(PanelMode::Own),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PanelMode::Relay => "relay",
            PanelMode::Gateway => "gateway",
            PanelMode::Own => "own",
        }
    }
}

pub const PANEL_MODES: &[PanelMode] = &[PanelMode::Relay, PanelMode::Gateway, PanelMode::Own];

/// Who serves each capability: `own,yandex`, `yandex`, `off` — the ROUTES syntax per capability.
pub const ROUTES: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "routes",
    scope: SettingScope::Global,
    description: "Маршруты возможностей",
    sensitive: false,
    parse: as_string_record,
};

/// The packages' own settings, by package id: the values the admin gave the fields its manifest declares
/// (src/plugins). A plugin reads them on every call, so a change applies at once; a value its next version no longer
/// takes falls back to the default.
pub const PACKAGE_SETTINGS: SettingDefinition<BTreeMap<String, serde_json::Map<String, Value>>> = SettingDefinition {
    key: "packages.settings",
    scope: SettingScope::Global,
    description: "Настройки приложений",
    sensitive: false,
    parse: |value| {
        let plugins = value.as_object()?;
        Some(plugins.iter().filter_map(|(id, values)| Some((id.clone(), values.as_object()?.clone()))).collect())
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeSetting {
    pub mode: PanelMode,
}

/// The station's mode, stored as `{mode}`.
pub const MODE: SettingDefinition<ModeSetting> = SettingDefinition {
    key: "gateway_mode",
    scope: SettingScope::Global,
    description: "Режим колонки",
    sensitive: false,
    parse: |value| {
        let mode = value.get("mode").and_then(Value::as_str).and_then(PanelMode::parse)?;
        Some(ModeSetting { mode })
    },
};

/// «Слух Яндекса» (docs/plans/yandex-ear.md): in «Вместе» with no ear of our own, Yandex recognizes the question and
/// waits, and our router answers it — our brain, our skills — or lets Yandex go on. On unless the owner turns it off;
/// off, Yandex both hears and answers, as before.
pub const YANDEX_EAR: SettingDefinition<bool> = SettingDefinition {
    key: "yandex_ear",
    scope: SettingScope::Global,
    description: "Слух Яндекса для своего ИИ",
    sensitive: false,
    parse: Value::as_bool,
};

/// How the household talks to DuoWave (docs/plans/mic-mode.md): through the station with Alice, or by this computer's
/// microphone — a place is left for other devices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TalkInput {
    #[default]
    Station,
    Computer,
}

impl TalkInput {
    pub fn parse(value: &str) -> Option<TalkInput> {
        match value {
            "station" => Some(TalkInput::Station),
            "computer" => Some(TalkInput::Computer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TalkInput::Station => "station",
            TalkInput::Computer => "computer",
        }
    }
}

/// «Как говорить с DuoWave», stored as `{input, before}`: `before` is the mode in force when the microphone was
/// chosen — the way back to the station returns it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceInput {
    pub input: TalkInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<PanelMode>,
}

/// Each gateway keeps its own (placement/transfer.rs, `LOCAL_SETTINGS`): the choice is the computer's, the station's
/// gateway never gets it.
pub const VOICE_INPUT: SettingDefinition<VoiceInput> = SettingDefinition {
    key: "voice_input",
    scope: SettingScope::Global,
    description: "Как говорить с DuoWave",
    sensitive: false,
    parse: |value| {
        let input = value.get("input").and_then(Value::as_str).and_then(TalkInput::parse)?;
        let before = value.get("before").and_then(Value::as_str).and_then(PanelMode::parse);
        Some(VoiceInput { input, before })
    },
};

/// Whose voice speaks our answers: Alice's (yandex), ours (`mixed`: ours for ours, Yandex's for its own), own, none.
pub const VOICE_PROVIDER: SettingDefinition<String> = SettingDefinition {
    key: "tts_provider",
    scope: SettingScope::User,
    description: "Чей голос отвечает",
    sensitive: false,
    parse: |value| as_one_of(value, &["yandex", "mixed", "own", "none"]),
};

/// Our voice, by catalog ids (`{modelId, voiceId}`).
pub const VOICE_CHOICE: SettingDefinition<serde_json::Map<String, Value>> = SettingDefinition {
    key: "tts_own",
    scope: SettingScope::User,
    description: "Наш голос",
    sensitive: false,
    parse: as_object,
};

/// The recognition model by its card id.
pub const ASR_MODEL: SettingDefinition<String> = SettingDefinition {
    key: "asr_model",
    scope: SettingScope::Global,
    description: "Модель распознавания",
    sensitive: false,
    parse: as_string,
};

/// Alisa's character: a preset card's id and the text it applied, or one's own text. A character chosen as a whole
/// (src/characters) keeps the text it gave then (`base`) and the tones chosen: the owner's edit is `text` differing
/// from `base`, and a newer text in its package is told, never applied unasked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub preset_id: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The position of each tone, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tones: Option<BTreeMap<String, usize>>,
}

impl Persona {
    /// A card's or one's own text, with nothing of a character's.
    pub fn plain(preset_id: Option<String>, text: String) -> Persona {
        Persona { preset_id, text, base: None, tones: None }
    }
}

pub const PERSONA: SettingDefinition<Persona> = SettingDefinition {
    key: "dialog_persona",
    scope: SettingScope::User,
    description: "Характер",
    sensitive: false,
    parse: |value| {
        let text = value.get("text").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())?;
        Some(Persona {
            preset_id: value.get("presetId").and_then(Value::as_str).map(str::to_string),
            text: text.to_string(),
            base: value.get("base").and_then(Value::as_str).map(str::to_string),
            tones: value.get("tones").and_then(Value::as_object).map(|tones| {
                tones.iter().filter_map(|(id, position)| Some((id.clone(), position.as_u64()? as usize))).collect()
            }),
        })
    },
};

/// The apps' history (docs/plans/package-history.md, «Где хранить историю»): `place` (`here`, `home`, `both`),
/// `budgetMb`, `days`, `steps`; a field left out goes by the device (plugins/history/gc.rs). Each gateway keeps its own:
/// a move leaves it (placement/transfer.rs, `LOCAL_SETTINGS`).
pub const PACKAGE_HISTORY: SettingDefinition<Value> = SettingDefinition {
    key: "package_history",
    scope: SettingScope::Global,
    description: "История приложений: где хранить и сколько памяти",
    sensitive: false,
    parse: |value| value.is_object().then(|| value.clone()),
};

/// The station's cloud hearing and voice (src/speech/cloud_speech.rs): `{hearing, voice}`, each `{service, model,
/// voice, url}`; the keys are in the safe. Each gateway keeps its own (placement/transfer.rs, `LOCAL_SETTINGS`).
pub const CLOUD_SPEECH: SettingDefinition<Value> = SettingDefinition {
    key: "cloud_speech",
    scope: SettingScope::Global,
    description: "Облачный слух и голос колонки",
    sensitive: false,
    parse: |value| value.is_object().then(|| value.clone()),
};

/// Whether the gateway reads the packages' catalog (src/plugins/catalog.rs): asking for it tells the site the
/// gateway's address, as the update checks do. On unless the owner turns it off.
pub const PACKAGES_CATALOG: SettingDefinition<bool> = SettingDefinition {
    key: "packages_catalog",
    scope: SettingScope::Global,
    description: "Загружать каталог приложений",
    sensitive: false,
    parse: Value::as_bool,
};

/// The hidden developer option «Показывать скрытые пакеты» (src/plugins/catalog.rs): the feeds' `unlisted` packages are
/// listed in MagicPlace, the characters' catalog and by voice too. Off unless the admin turns it on.
pub const PACKAGES_SHOW_HIDDEN: SettingDefinition<bool> = SettingDefinition {
    key: "packages.show_hidden",
    scope: SettingScope::Global,
    description: "Показывать скрытые пакеты",
    sensitive: false,
    parse: Value::as_bool,
};

/// The characters' memory (src/memory, docs/plans/memory.md): on unless the owner turns it off. Off, nothing that
/// grew from talk — the household and the characters' personal records — goes into the prompt or is written; a
/// character's world and package knowledge are not part of this switch.
pub const MEMORY: SettingDefinition<bool> = SettingDefinition {
    key: "memory",
    scope: SettingScope::Global,
    description: "Память персонажей",
    sensitive: false,
    parse: Value::as_bool,
};

/// «Запоминать мимоходом»: the model remembers what is worth keeping without being asked (on by default, the
/// owner's decision of 12.11); off, it remembers only on «запомни».
pub const MEMORY_IN_PASSING: SettingDefinition<bool> = SettingDefinition {
    key: "memory_in_passing",
    scope: SettingScope::Global,
    description: "Запоминать мимоходом",
    sensitive: false,
    parse: Value::as_bool,
};

/// The memory's pause: until when nothing new is written (an RFC 3339 time), `manual` until someone turns it back
/// on, empty when there is none. What is known is still used.
pub const MEMORY_PAUSE: SettingDefinition<String> = SettingDefinition {
    key: "memory_pause",
    scope: SettingScope::Global,
    description: "Пауза памяти",
    sensitive: false,
    parse: as_string,
};

/// When the night's work starts (docs/plans/memory.md, «Сон»): "HH:MM" on the station's clock, 04:00 when unset.
/// The sleep runs from then till morning, and only when one of its parts that is on has work.
pub const SLEEP_TIME: SettingDefinition<String> = SettingDefinition {
    key: "sleep_time",
    scope: SettingScope::Global,
    description: "Сон: время",
    sensitive: false,
    parse: |value| {
        let text = value.as_str()?;
        let (hours, minutes) = text.split_once(':')?;
        let (hours, minutes) = (hours.parse::<u32>().ok()?, minutes.parse::<u32>().ok()?);
        (hours < 24 && minutes < 60 && text.len() == 5).then(|| text.to_string())
    },
};

/// «Рефлексия»: the sleep's part that thinks the day over. Off by default (the owner's decision of 12.11).
pub const MEMORY_REFLECTION: SettingDefinition<bool> = SettingDefinition {
    key: "memory_reflection",
    scope: SettingScope::Global,
    description: "Рефлексия",
    sensitive: false,
    parse: Value::as_bool,
};

/// The reflection's «Порядок»: repeats glued, what a newer fact contradicts closed, other names linked, «завтра»
/// turned into dates, the past put into history, importance recounted. On by default once the reflection is on.
pub const MEMORY_REFLECTION_ORDER: SettingDefinition<bool> = SettingDefinition {
    key: "memory_reflection_order",
    scope: SettingScope::Global,
    description: "Рефлексия: порядок",
    sensitive: false,
    parse: Value::as_bool,
};

/// The reflection's «Итог дня»: a short summary for the house and a diary entry for each character who talked that
/// day. On by default once the reflection is on.
pub const MEMORY_REFLECTION_DAY: SettingDefinition<bool> = SettingDefinition {
    key: "memory_reflection_day",
    scope: SettingScope::Global,
    description: "Рефлексия: итог дня",
    sensitive: false,
    parse: Value::as_bool,
};

/// «Модель памяти»: which model thinks for the memory — the reflection at night now, the разбор of a conversation
/// later — apart from «Мозг» (the owner, 12.11: «чтоб можно было память отдельно настраивать … домашний или
/// облачный, какой хочешь»). A model at home by default.
pub const MEMORY_MODEL: SettingDefinition<MemoryModel> = SettingDefinition {
    key: "memory_model",
    scope: SettingScope::Global,
    description: "Модель памяти",
    sensitive: false,
    parse: as_serde,
};

/// The memory's model as chosen: `{"mode": "home"}`, `{"mode": "brain"}` or `{"mode": "pick", "provider", "model"}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum MemoryModel {
    /// A model at home, whichever is there: the talk stays in the house; none — the memory waits.
    #[default]
    Home,
    /// The «Мозг»'s smart model, wherever it is, the cloud too.
    Brain,
    /// One model of «Мозг»'s providers, at home or in the cloud.
    Pick { provider: String, model: String },
}

/// What a pending voice waits for: an engine's install, a voice file's download, or an imprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWait {
    /// engine, file, imprint.
    pub kind: String,
    pub id: String,
}

/// How a character's voice is made when its package has no snapshot for the installed engine: an imprint from the
/// package's reference and its known words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRecipe {
    /// The voice's id in the voice lab: `c-<package>-<character>-<engine>`.
    pub voice: String,
    pub engine: String,
    /// The reference recording (a file of the package) and its words.
    pub reference: String,
    pub text: String,
    pub name: String,
    pub package: String,
    pub version: String,
    pub character: String,
}

/// A voice chosen before what it needs is there (src/models): it is switched on when the engine is installed, the file
/// downloaded or the imprint taken. Any other choice of a voice forgets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePending {
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    pub wait_for: PendingWait,
    /// The owner said «всё равно» to tight memory already.
    #[serde(default)]
    pub anyway: bool,
    /// waiting, tight (memory: it waits for «всё равно?»), failed.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Tight memory as numbers: `reason` is written again from them whenever it is read, in the reader's language
    /// (clean-path check 19.11: a reason kept in the words of the moment it was found read «ОЗУ впритык: Memory is
    /// tight: …» in a Russian panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryTight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<VoiceRecipe>,
    pub since: String,
}

/// Why a heavy start waits on the owner's «всё равно», as numbers (MB) — its words are written when they are read
/// (voice_lab/memory_policy.rs, `words`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MemoryTight {
    /// The system is short of memory already.
    Pressure,
    /// It pages in from the disk now.
    Swap,
    /// Short of video memory.
    Video { free_mb: u64, need_mb: u64 },
    /// Room for it, but not beside what is kept for the household.
    Reserve { free_mb: u64, need_mb: u64, reserve_mb: u64 },
    /// No room for it at all: the computer will page.
    Short { free_mb: u64, need_mb: u64 },
}

pub const VOICE_PENDING: SettingDefinition<VoicePending> = SettingDefinition {
    key: "voice_pending",
    scope: SettingScope::Global,
    description: "Голос, который включится, когда встанет его модель",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// DuoWave's invitation code (`s_…`): sent with every DuoWave sign-in until one brings a key — the gift comes only with
/// the person's very first sign-in at my.duowave.app, so the code must ride with it. Set by the panel; a sign-in that
/// brought a key clears it.
pub const DUOWAVE_INVITE: SettingDefinition<String> = SettingDefinition {
    key: "duowave_invite",
    scope: SettingScope::Global,
    description: "Код приглашения DuoWave",
    sensitive: false,
    parse: |value| value.as_str().filter(|code| crate::oauth::invite_code(code)).map(str::to_string),
};

/// The secret the panel's tokens are signed with: made at the first start, never shown.
pub const AUTH_SECRET: SettingDefinition<String> = SettingDefinition {
    key: "panel_secret",
    scope: SettingScope::Global,
    description: "Секрет подписи входа",
    sensitive: true,
    parse: |value| value.as_str().filter(|secret| secret.len() >= 32).map(str::to_string),
};

/// Who the station's voice is when nobody is recognised: a role and the household's own restrictions. A member by
/// default — the station answered everyone before there were people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Household {
    pub role: Role,
    #[serde(default)]
    pub restrictions: Restrictions,
}

pub const HOUSEHOLD: SettingDefinition<Household> = SettingDefinition {
    key: "household",
    scope: SettingScope::Global,
    description: "Права голоса дома, пока говорящий не узнан",
    sensitive: false,
    parse: |value| {
        let object = value.as_object()?;
        let restrictions = object.get("restrictions").and_then(Value::as_object);
        let list = |name: &str| restrictions.and_then(|r| r.get(name)).and_then(as_string_list);
        Some(Household {
            role: normalize_role(object.get("role").and_then(Value::as_str).unwrap_or("")),
            restrictions: Restrictions { allow: list("allow"), deny: list("deny") },
        })
    },
};

/// A station's voice server addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAddresses {
    pub url: String,
    pub quic: String,
}

/// What the gateway remembers about a station it connected from the panel, by the station's address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationLink {
    /// The station's SSH host key as first seen: a different one later stops the gateway (another station, a reflash).
    pub fingerprint: String,
    /// Its voice server addresses without a gateway: restore.sh and «Вернуть к Яндексу».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<LinkAddresses>,
    /// What it had right before the last «Подключить», another gateway included: «Вернуть как было».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<LinkAddresses>,
    /// What it was given: ws://<gateway>/uni.ws.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// The station that came after the switch, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<String>,
}

/// Stations connected from the panel, by address (src/stations/connect): their keys and their way back.
pub const STATION_LINKS: SettingDefinition<BTreeMap<String, StationLink>> = SettingDefinition {
    key: "station_links",
    scope: SettingScope::Global,
    description: "Колонки, подключённые из пульта",
    sensitive: false,
    parse: |value| {
        let object = value.as_object()?;
        Some(
            object
                .iter()
                .filter_map(|(address, entry)| {
                    serde_json::from_value::<StationLink>(entry.clone()).ok().map(|link| (address.clone(), link))
                })
                .collect(),
        )
    },
};

/// The household's language (docs/plans/i18n.md): a BCP 47 primary tag, `ru`, `en`… What runs with nobody at a
/// screen speaks it — the apps' code (`host.i18n.locale`), schedules, notices; a panel without its own pick shows it.
pub const LANGUAGE: SettingDefinition<String> = SettingDefinition {
    key: "language",
    scope: SettingScope::Global,
    description: "Язык дома",
    sensitive: false,
    parse: |value| value.as_str().and_then(crate::locale::normalize),
};

/// The household's country as gateway 0.13.0 kept it (the panel asked): an ISO 3166-1 alpha-2 code or `none`. No longer
/// asked or written: the app «VPN» keeps the answer as its own setting, and an answer found here moves there once
/// (src/locale/country.rs).
pub const HOME_COUNTRY: SettingDefinition<String> = SettingDefinition {
    key: "home_country",
    scope: SettingScope::Global,
    description: "Страна дома",
    sensitive: false,
    parse: |value| value.as_str().and_then(crate::locale::country::normalize),
};

/// «Выход в интернет» (src/egress, docs/plans/own-server.md): the mode the routes are decided by — `off`, `blocked`
/// («Где не работает», the default), `all`, `only` («Только выбранные»). Since 16.11 it follows the switch, the pick
/// and «напрямую то, что работает из страны дома» below; an older panel's `mode` still sets it as it did.
pub const EGRESS_MODE: SettingDefinition<String> = SettingDefinition {
    key: "egress.mode",
    scope: SettingScope::Global,
    description: "Выход в интернет: простой режим",
    sensitive: false,
    parse: |value| as_one_of(value, &["off", "blocked", "all", "only"]),
};

/// «Режим VPN для DuoWave»: `all` — everyone through the server, `except` — everyone but the rows ticked (their route
/// `direct`), `only` — only the rows ticked (their route `server` or a connection). Kept while the switch is off.
pub const EGRESS_PICK: SettingDefinition<String> = SettingDefinition {
    key: "egress.pick",
    scope: SettingScope::Global,
    description: "Выход в интернет: кто идёт через сервер",
    sensitive: false,
    parse: |value| as_one_of(value, &["all", "except", "only"]),
};

/// «Открывать напрямую то, что работает из страны дома»: on — `blocked`, off — `all`. Kept while the switch is off.
pub const EGRESS_HOME_DIRECT: SettingDefinition<bool> = SettingDefinition {
    key: "egress.home_direct",
    scope: SettingScope::Global,
    description: "Выход в интернет: работающее из страны дома — напрямую",
    sensitive: false,
    parse: Value::as_bool,
};

/// «Если VPN не отвечает», for every row without its own word: `safe` — keyed rows and apps wait, Alice goes directly
/// (the default), `wait` — everyone waits, `direct` — everyone goes directly.
pub const EGRESS_ON_DOWN_ALL: SettingDefinition<String> = SettingDefinition {
    key: "egress.on_down_all",
    scope: SettingScope::Global,
    description: "Выход в интернет: если VPN не отвечает",
    sensitive: false,
    parse: |value| as_one_of(value, &["safe", "wait", "direct"]),
};

/// The connection «Всё через сервер» and «Авто» go through first; none — the first that answers.
pub const EGRESS_CONNECTION: SettingDefinition<String> = SettingDefinition {
    key: "egress.connection",
    scope: SettingScope::Global,
    description: "Выход в интернет: подключение по умолчанию",
    sensitive: false,
    parse: as_string,
};

/// The addresses the person typed («Свои адреса»): host names, `*.example.com` for subdomains.
pub const EGRESS_CUSTOM: SettingDefinition<Vec<String>> = SettingDefinition {
    key: "egress.custom",
    scope: SettingScope::Global,
    description: "Выход в интернет: свои адреса",
    sensitive: false,
    parse: as_string_list,
};

/// The connections last reported, without their entries: after a restart the rows that wait for a server keep
/// waiting until the app speaks again (src/egress/connections.rs).
pub const EGRESS_CONNECTIONS: SettingDefinition<Vec<crate::egress::connections::Kept>> = SettingDefinition {
    key: "egress.connections",
    scope: SettingScope::Global,
    description: "Выход в интернет: известные подключения",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The rows the person has seen in the table: the others are marked «новое».
pub const EGRESS_SEEN: SettingDefinition<Vec<String>> = SettingDefinition {
    key: "egress.seen",
    scope: SettingScope::Global,
    description: "Выход в интернет: просмотренные строки",
    sensitive: false,
    parse: as_string_list,
};

/// A row's own route (app_settings, by the row's id): `auto`, `direct`, `server` (the location chosen for all) or a
/// connection's id.
pub const EGRESS_ROUTE: SettingDefinition<String> = SettingDefinition {
    key: "egress.route",
    scope: SettingScope::App,
    description: "Выход в интернет: маршрут строки",
    sensitive: false,
    parse: |value| value.as_str().and_then(crate::egress::rules::Route::parse).map(|route| route.as_text().to_string()),
};

/// What a row does when its connection does not answer: `wait` or `direct`.
pub const EGRESS_ON_DOWN: SettingDefinition<String> = SettingDefinition {
    key: "egress.on_down",
    scope: SettingScope::App,
    description: "Выход в интернет: если сервер не отвечает",
    sensitive: false,
    parse: |value| as_one_of(value, &["wait", "direct"]),
};

/// What this home learned of a row by trying: directly it did not open. Each side keeps its own (it is about this
/// home's network): it does not travel with the settings package (placement/transfer.rs, `LOCAL_APP_SETTINGS`).
pub const EGRESS_LEARNED: SettingDefinition<crate::egress::rules::Learned> = SettingDefinition {
    key: "egress.learned",
    scope: SettingScope::App,
    description: "Выход в интернет: что дом узнал сам",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The journal of requests, in days: 0 keeps none (the dashboard still shows them live).
pub const KEEP_REQUESTS: SettingDefinition<u32> = SettingDefinition {
    key: "requests_keep_days",
    scope: SettingScope::Global,
    description: "Сколько дней хранить журнал запросов",
    sensitive: false,
    parse: |value| value.as_u64().filter(|days| *days <= 3650).map(|days| days as u32),
};

/// A profile's model: the panel's provider and its model id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrainPick {
    pub provider: String,
    pub model: String,
}

/// Which model answers (src/dialog/brain): the fast profile and the smart one, each from any provider added in the
/// panel, and Claude's thinking depth. Without `fast` the .env model answers; an unset smart follows the fast one.
/// `backup` answers when the chosen model gave no answer (the ChatGPT plan's limit, no connection, a refusal): a panel
/// provider's model, or `env` for the .env one; unset — no backup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<BrainPick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<BrainPick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<BrainPick>,
    /// How the backup came (docs/plans/home-and-helpers.md, default 6): unset — a person chose it, or, with no backup,
    /// «пусть выберет шлюз» (decision 18). A gateway without the block never writes it: its «не нужна» is not kept.
    #[serde(default, rename = "backupBy", skip_serializing_if = "Option::is_none")]
    pub backup_by: Option<BackupBy>,
}

/// The gateway set the backup by itself, or a person said «не нужна» — then the gateway sets none again until
/// `backup: "auto"` («пусть выберет шлюз») hands the choice back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupBy {
    Gateway,
    Declined,
}

/// The workshop agent's own model (src/plugins/workshop/agent.rs): a provider of the panel and its model; unset — the
/// smart profile of «Мозг», as before. Only the agent reads it: the household's conversation keeps «Мозг».
/// «Ставить голосом» (docs/plans/audio-interface.md, the owner's decisions 2 and 8): a «да» said to a station after
/// «На стол или примерить?» installs the thing, named rights and hosts included; dangerous rights stay the panel's.
/// MagicPlace's «Поставить «Ленту»?» and «Обновить всё?» obey it too (src/talk/store.rs, docs/plans/magicplace.md).
/// On unless the owner turns it off: then «Одобрите в пульте», as before.
pub const TALK_DESK_BY_VOICE: SettingDefinition<bool> = SettingDefinition {
    key: "talk.desk_by_voice",
    scope: SettingScope::Global,
    description: "Ставить голосом",
    sensitive: false,
    parse: Value::as_bool,
};

pub const WORKSHOP_AGENT: SettingDefinition<BrainPick> = SettingDefinition {
    key: "workshop.agent",
    scope: SettingScope::Global,
    description: "Модель агента мастерской",
    sensitive: false,
    parse: |value| {
        let provider = value.get("provider")?.as_str().filter(|provider| !provider.is_empty())?;
        let model = value.get("model")?.as_str().filter(|model| !model.is_empty())?;
        Some(BrainPick { provider: provider.to_string(), model: model.to_string() })
    },
};

pub const BRAIN: SettingDefinition<Brain> = SettingDefinition {
    key: "brain",
    scope: SettingScope::Global,
    description: "Модель для разговора",
    sensitive: false,
    parse: |value| {
        let object = value.as_object()?;
        let pick = |name: &str| {
            let item = object.get(name)?.as_object()?;
            let provider = item.get("provider")?.as_str()?;
            let model = item.get("model")?.as_str().filter(|model| !model.is_empty())?;
            Some(BrainPick { provider: provider.to_string(), model: model.to_string() })
        };
        Some(Brain {
            fast: pick("fast"),
            smart: pick("smart"),
            effort: object.get("effort").and_then(|effort| as_one_of(effort, crate::config::LLM_EFFORTS)),
            backup: pick("backup"),
            backup_by: object.get("backupBy").and_then(|by| serde_json::from_value(by.clone()).ok()),
        })
    },
};

// The connectors the panel configures (src/connectors/settings.rs): one setting each for the plain fields; tokens,
// passwords and keys are secrets (src/secrets), never here.

/// A stored value read with serde: what does not fit reads as absent.
fn as_serde<T: serde::de::DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

fn on() -> bool {
    true
}

/// A connector's contact: a name people say and where it goes — a Telegram chat, a phone number or an extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEntry {
    pub name: String,
    pub to: String,
}

/// Home Assistant: its address at home; the token is a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeAssistantSetting {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
}

impl Default for HomeAssistantSetting {
    fn default() -> Self {
        HomeAssistantSetting { enabled: true, url: String::new() }
    }
}

/// The Telegram bot: whom it knows and where the station's alerts go; the token is a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramSetting {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub contacts: Vec<ContactEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_chat: Option<String>,
}

impl Default for TelegramSetting {
    fn default() -> Self {
        TelegramSetting { enabled: true, contacts: Vec::new(), alert_chat: None }
    }
}

fn home_room() -> String {
    "home".into()
}

/// LiveKit: the server for the gateway, the address phones reach from outside, the call page and the room; the API
/// key and secret are secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveKitSetting {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(default = "home_room")]
    pub room: String,
}

impl Default for LiveKitSetting {
    fn default() -> Self {
        LiveKitSetting { enabled: true, url: String::new(), public_url: None, page_url: None, room: home_room() }
    }
}

fn ari_app() -> String {
    "duowave".into()
}

fn rtp_host() -> String {
    "127.0.0.1".into()
}

fn rtp_port() -> u16 {
    40_000
}

/// SIP through Asterisk ARI: where ARI answers, who we are there, our RTP and the contacts; the password is a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SipSetting {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "ari_app")]
    pub app: String,
    #[serde(default = "rtp_host")]
    pub rtp_host: String,
    #[serde(default = "rtp_port")]
    pub rtp_port: u16,
    #[serde(default)]
    pub contacts: Vec<ContactEntry>,
}

impl Default for SipSetting {
    fn default() -> Self {
        SipSetting {
            enabled: true,
            url: String::new(),
            user: String::new(),
            app: ari_app(),
            rtp_host: rtp_host(),
            rtp_port: rtp_port(),
            contacts: Vec::new(),
        }
    }
}

/// The weather's usual place; none: the city of the station's time zone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherSetting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// A news feed: its address, its name for people, the catalog's topic and entry when it came from there, and
/// whether it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeedEntry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default = "on")]
    pub enabled: bool,
}

/// The news: our feeds, from the catalog and one's own; none: news is not ours.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsSetting {
    #[serde(default)]
    pub feeds: Vec<NewsFeedEntry>,
}

pub const CONNECTOR_HOME_ASSISTANT: SettingDefinition<HomeAssistantSetting> = SettingDefinition {
    key: "connector_home_assistant",
    scope: SettingScope::Global,
    description: "Подключение Home Assistant",
    sensitive: false,
    parse: as_serde,
};

pub const CONNECTOR_TELEGRAM: SettingDefinition<TelegramSetting> = SettingDefinition {
    key: "connector_telegram",
    scope: SettingScope::Global,
    description: "Подключение Telegram",
    sensitive: false,
    parse: as_serde,
};

pub const CONNECTOR_LIVEKIT: SettingDefinition<LiveKitSetting> = SettingDefinition {
    key: "connector_livekit",
    scope: SettingScope::Global,
    description: "Подключение LiveKit",
    sensitive: false,
    parse: as_serde,
};

pub const CONNECTOR_SIP: SettingDefinition<SipSetting> = SettingDefinition {
    key: "connector_sip",
    scope: SettingScope::Global,
    description: "Подключение телефонии (SIP)",
    sensitive: false,
    parse: as_serde,
};

pub const CONNECTOR_WEATHER: SettingDefinition<WeatherSetting> = SettingDefinition {
    key: "connector_weather",
    scope: SettingScope::Global,
    description: "Город для погоды",
    sensitive: false,
    parse: as_serde,
};

pub const CONNECTOR_NEWS: SettingDefinition<NewsSetting> = SettingDefinition {
    key: "connector_news",
    scope: SettingScope::Global,
    description: "Ленты новостей",
    sensitive: false,
    parse: as_serde,
};

/// Who chose the mode in force: a person, or the gateway itself when its home server went (src/placement/state.rs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeBy {
    #[default]
    Person,
    Auto,
}

/// The gateway's own switch, kept until a person decides or the reason passes: from which mode, to which, why, when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSwitch {
    pub from: PanelMode,
    pub to: PanelMode,
    pub reason: String,
    pub at: String,
}

/// Where the main settings live when the gateway is on the station: there (a copy on the computer), or on the
/// computer (the station works on the last copy it was sent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsHome {
    #[default]
    Station,
    Pc,
}

/// The safety net's last act on the station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyCase {
    /// previous-version, previous-address.
    pub what: String,
    pub at: String,
}

/// The gateway's state in one record (docs/plans/station-placement.md, section 8): who chose the mode and the
/// gateway's own switch, the safety net, where the main settings live. What is not in it is the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GatewayState {
    pub mode_by: ModeBy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoSwitch>,
    /// The safety net of the station's service: on unless turned off.
    pub safety: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_last: Option<SafetyCase>,
    pub settings_home: SettingsHome,
}

impl Default for GatewayState {
    fn default() -> GatewayState {
        GatewayState {
            mode_by: ModeBy::Person,
            auto: None,
            safety: true,
            safety_last: None,
            settings_home: SettingsHome::Station,
        }
    }
}

pub const GATEWAY_STATE: SettingDefinition<GatewayState> = SettingDefinition {
    key: "gateway_state",
    scope: SettingScope::Global,
    description: "Состояние шлюза: кто выбрал режим, страховка, где главные настройки",
    sensitive: false,
    parse: as_serde,
};

/// What the computer's gateway knows of the station's (src/placement): where the gateway runs for it, how to reach
/// it, and the household's choices about it. The pair's key is a secret, kept apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationPlacement {
    pub placement: crate::core::availability::Placement,
    /// The station's address on the home network.
    pub address: String,
    /// The port its own gateway listens on.
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default)]
    pub settings_home: SettingsHome,
    /// Update the station's gateway with this one, without asking.
    #[serde(default)]
    pub auto_update: bool,
    /// Installed from this panel: this computer updates it.
    #[serde(default)]
    pub managed: bool,
    /// The key on this computer the station's gateway reaches its home server with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_at: Option<String>,
}

/// The one placement of the computer before each station had its own: read once, moved into STATION_PLACEMENTS.
pub const STATION_PLACEMENT: SettingDefinition<StationPlacement> = SettingDefinition {
    key: "station_placement",
    scope: SettingScope::Global,
    description: "Где работает шлюз колонки (прежняя запись)",
    sensitive: false,
    parse: as_serde,
};

/// Where each station's gateway runs, by the station: its device, else «at:<address>» for a gateway linked by hand
/// (docs/plans/placement-per-station.md).
pub const STATION_PLACEMENTS: SettingDefinition<BTreeMap<String, StationPlacement>> = SettingDefinition {
    key: "station_placements",
    scope: SettingScope::Global,
    description: "Где работает шлюз каждой колонки",
    sensitive: false,
    parse: as_serde,
};

/// «Обновить компьютер и колонку до X» (owner 20.11): the computer gives a station only its own build, so a newer
/// release goes to the computer first and the station follows once the computer runs it. Kept through the computer's
/// restart into the new version (src/placement/watch.rs, `follow_up`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationUpdateAfter {
    /// The version the station is to get.
    pub version: String,
    /// When it was asked: an old request is dropped, not acted on days later.
    pub at: String,
}

/// The stations to update once this computer runs the version asked for, by the station (its key, as in
/// STATION_PLACEMENTS).
pub const STATION_UPDATES_AFTER: SettingDefinition<BTreeMap<String, StationUpdateAfter>> = SettingDefinition {
    key: "station_updates_after",
    scope: SettingScope::Global,
    description: "Колонки, которые обновятся следом за компьютером",
    sensitive: false,
    parse: as_serde,
};

/// The station's gateway about the station it runs on, as the computer that moved it there knows it: its device and the
/// name the household gave it there («Детская»). The station's own list of stations may know it by another tag, and a
/// new tag is «Колонка» there; a move brings this (src/placement/transfer.rs, `MOVE_SETS`), /health says it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationSelf {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

pub const STATION_SELF: SettingDefinition<StationSelf> = SettingDefinition {
    key: "station_self",
    scope: SettingScope::Global,
    description: "Колонка, на которой работает этот шлюз",
    sensitive: false,
    parse: as_serde,
};

/// The home this gateway belongs to (docs/plans/home-and-helpers.md, «Дом»): every gateway of a family is in one and
/// shares its key (the secret `home:key`, src/core/identity/home.rs). It travels with the settings package from schema 4
/// on (src/placement/transfer.rs, `SINCE`), so a station whose settings the computer keeps joins the computer's home with
/// the first package (the plan's default 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeIdentity {
    pub id: String,
    /// What the household calls it; none — «Дом». Not shown in block 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

pub const HOME_IDENTITY: SettingDefinition<HomeIdentity> = SettingDefinition {
    key: "home_identity",
    scope: SettingScope::Global,
    description: "Дом этого шлюза",
    sensitive: false,
    parse: |value| serde_json::from_value::<HomeIdentity>(value.clone()).ok().filter(|home| !home.id.trim().is_empty()),
};

/// This gateway itself: an id of its own and the name a person gave it (src/core/identity/home.rs). Each gateway's own,
/// never in the settings package (`transfer::LOCAL_SETTINGS`): the station and the computer of one home keep their ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewaySelf {
    pub id: String,
    /// A person's name for it; none — the default (`Identity::default_name`), worked out when read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

pub const GATEWAY_SELF: SettingDefinition<GatewaySelf> = SettingDefinition {
    key: "gateway_self",
    scope: SettingScope::Global,
    description: "Этот шлюз: его номер и имя",
    sensitive: false,
    parse: |value| serde_json::from_value::<GatewaySelf>(value.clone()).ok().filter(|me| !me.id.trim().is_empty()),
};

/// The station's home server (src/home_server): the computer's address. Its key is a secret, kept apart. Read one more
/// version: the helpers' entry `home-computer` took its place, and the two are kept in step (src/helpers/legacy.rs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeServerSetting {
    pub url: String,
    /// «ПК помогает колонке»: off — the address and the key stay, the station works alone (its placement «На
    /// колонке»); on — «Колонка + ПК». A move always brings both; the switch only says whether to lean on it.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// What the station does not take from the computer while it helps (`home_server::SERVES`): "asr" (hearing),
    /// "tts" (voice), "llm" (the model), "messages" (the computer's Telegram bot) — the helper's «Что даю колонке»
    /// (docs/plans/desk.md, stage 4). Empty — it takes all of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without: Vec<String>,
}

fn yes() -> bool {
    true
}

pub const CONNECTOR_HOME_SERVER: SettingDefinition<HomeServerSetting> = SettingDefinition {
    key: "connector_home_server",
    scope: SettingScope::Global,
    description: "Домашний сервер колонки",
    sensitive: false,
    parse: as_serde,
};

/// The helpers in the household's order (src/helpers, docs/plans/home-and-helpers.md): the computer at home, one's own
/// server, the DuoWave cloud. Shared by the home's gateways: it travels with the settings package from schema 4 on,
/// but for each gateway's own entry `home-computer` (src/placement/transfer.rs, `OWN_PARTS`). Their keys are secrets,
/// `helper:<id>:key`.
pub const HELPERS: SettingDefinition<Vec<crate::helpers::HelperSetting>> = SettingDefinition {
    key: "helpers",
    scope: SettingScope::Global,
    description: "Турбо",
    sensitive: false,
    parse: crate::helpers::parse_helpers,
};

/// A room of the home (docs/plans/home-and-devices.md, «Комнаты», stage 2): what may differ in one device's room —
/// Alice's mode, the volume, who answers here; none — as in the whole home. The name and the room are the device's
/// own (its record in `stations`). Kept at the centre, by device.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomSetting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PanelMode>,
    /// 0–10, as the station's own scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// The character who answers here, by id; its words as they were when chosen (like the home's `dialog_persona`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<Persona>,
}

impl RoomSetting {
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.volume.is_none() && self.character.is_none()
    }
}

pub const ROOMS: SettingDefinition<BTreeMap<String, RoomSetting>> = SettingDefinition {
    key: "device_rooms",
    scope: SettingScope::Global,
    description: "Комнаты",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// «Что может уходить из дома» (src/helpers): «Только дома», «Дом и моё облако», «Любые облака» (the default). Travels
/// with the settings package.
pub const AWAY: SettingDefinition<crate::helpers::Away> = SettingDefinition {
    key: "away",
    scope: SettingScope::Global,
    description: "Что может уходить из дома",
    sensitive: false,
    parse: as_serde,
};

/// The certificate a linked helper showed at its first meeting (src/helpers/link.rs): its DER's SHA-256, or — a helper
/// behind a proxy with a real domain — the DNS name its public certificate is checked for. A helper whose
/// certificate later differs is refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperPin {
    /// The certificate's SHA-256, 64 hex characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The DNS name of a publicly valid certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<String>,
    pub since: String,
}

/// The pins of the linked helpers, by entry id. They travel with the list (schema 4), but for the pair's door
/// `home-computer` (src/placement/transfer.rs, `OWN_PARTS`).
pub const HELPER_PINS: SettingDefinition<BTreeMap<String, HelperPin>> = SettingDefinition {
    key: "helper_pins",
    scope: SettingScope::Global,
    description: "Сертификаты Турбо",
    sensitive: false,
    parse: |value| {
        let pins: BTreeMap<String, Value> = serde_json::from_value(value.clone()).ok()?;
        Some(
            pins.into_iter()
                .filter_map(|(id, pin)| Some((id, serde_json::from_value::<HelperPin>(pin).ok()?)))
                .filter(|(_, pin)| pin.sha256.is_some() || pin.public.is_some())
                .collect(),
        )
    },
};

/// A gateway of a home this computer helps, as it named itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenGateway {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// A home this computer helps over the link (src/helpers/clients.rs): who it said it is, whether its home key proved
/// it is this computer's own home, its gateways (at most 16), what it is given. Its link key lives apart, hashed
/// (`helping:client:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperClient {
    pub home: crate::helpers::contract::HelperIdentity,
    #[serde(default)]
    pub own_home: bool,
    #[serde(default)]
    pub gateways: BTreeMap<String, SeenGateway>,
    #[serde(default, deserialize_with = "crate::helpers::abilities_lenient")]
    pub gives: Vec<crate::helpers::contract::Ability>,
    pub since: String,
    #[serde(default)]
    pub confirmed: bool,
    /// What its pairing bound (`Binding::stored`): its later links prove over it. None for a home paired before it was
    /// kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    /// `gives` was written knowing the abilities of contract 3 (devices, folders): before it, its own home gets them by
    /// default and another home does not (docs/plans/turbo-devices.md, owner 20.11, 9).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub v3: bool,
}

/// Who this computer helps over the link, by client id (`link:<12 hex>`). Each computer's own
/// (`transfer::LOCAL_SETTINGS`).
pub const HELPER_CLIENTS: SettingDefinition<BTreeMap<String, HelperClient>> = SettingDefinition {
    key: "helper_clients",
    scope: SettingScope::Global,
    description: "Кому помогает этот компьютер",
    sensitive: false,
    parse: |value| {
        let clients: BTreeMap<String, Value> = serde_json::from_value(value.clone()).ok()?;
        Some(
            clients
                .into_iter()
                .filter_map(|(id, client)| Some((id, serde_json::from_value::<HelperClient>(client).ok()?)))
                .collect(),
        )
    },
};

/// «Адрес снаружи»: the address other homes reach this computer's link at — a domain of the owner's proxy, or a
/// forwarded port. Each computer's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperOutside {
    pub url: String,
}

pub const HELPER_OUTSIDE: SettingDefinition<HelperOutside> = SettingDefinition {
    key: "helper_outside",
    scope: SettingScope::Global,
    description: "Адрес снаружи",
    sensitive: false,
    parse: as_serde,
};

/// The bus's links (src/bus, docs/plans/bus.md): who reaches whom and how far. Like every setting not in
/// `LOCAL_SETTINGS` they travel with the settings package, so a move keeps them.
pub const BUS_LINKS: SettingDefinition<Vec<crate::bus::links::Link>> = SettingDefinition {
    key: "bus.links",
    scope: SettingScope::Global,
    description: "Связи приложений",
    sensitive: false,
    parse: crate::bus::links::parse_links,
};

/// The house's mode and each participant's own: «Отдельно», «По правилам», «Свободно».
pub const BUS_MODES: SettingDefinition<crate::bus::links::Modes> = SettingDefinition {
    key: "bus.modes",
    scope: SettingScope::Global,
    description: "Режимы связей",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The bus's switches: asking in words through a cloud model without a person (off).
pub const BUS_OPTIONS: SettingDefinition<crate::bus::links::Options> = SettingDefinition {
    key: "bus.options",
    scope: SettingScope::Global,
    description: "Связи: настройки",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The owner's words on dangerous handles: «всегда можно», a mark taken off or put on.
pub const BUS_DANGERS: SettingDefinition<Vec<crate::bus::danger::DangerWord>> = SettingDefinition {
    key: "bus.dangers",
    scope: SettingScope::Global,
    description: "Опасные действия",
    sensitive: false,
    parse: |value| {
        let list = value.as_array()?;
        Some(list.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
    },
};

/// A work folder (docs/plans/router-and-works.md, block A, 6; src/works): a folder on this computer the owner added in
/// the panel for the helper to work in. `kind` — how it is kept: only `git` in the first version (decision 9).
/// `agent` and `model` — its agent and the model for it; empty — the seat «Исполнитель» (Claude Code and its own
/// choice). `maxUsd` — the ceiling of one run. `prepare` — commands for a fresh work copy; `checks` — the folder's own
/// checks after a run; `allow` — what the agent may run without a question, as Claude Code writes its rules
/// (`Bash(npm test *)`; `git push` too only when listed, decision 21).
#[cfg(feature = "pc")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default = "git_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd: Option<f64>,
    #[serde(default)]
    pub prepare: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

#[cfg(feature = "pc")]
fn git_kind() -> String {
    "git".into()
}

/// The work folders: each computer's own — its paths stay here (placement/transfer.rs, `LOCAL_SETTINGS`).
#[cfg(feature = "pc")]
pub const WORK_FOLDERS: SettingDefinition<Vec<WorkFolder>> = SettingDefinition {
    key: "work_folders",
    scope: SettingScope::Global,
    description: "Рабочие папки",
    sensitive: false,
    parse: |value| {
        let list = value.as_array()?;
        Some(list.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
    },
};

/// Where Claude Code's program is, when not on the search path (src/works/harness/claude_code.rs). Local.
#[cfg(feature = "pc")]
pub const HARNESS_CLAUDE_PATH: SettingDefinition<String> = SettingDefinition {
    key: "harness_claude_path",
    scope: SettingScope::Global,
    description: "Где программа Claude Code",
    sensitive: false,
    parse: as_string,
};

/// How Claude Code pays for a run: `terminal` — its own sign-in, `key` — the gateway's key. Not set — «как в
/// терминале», and the panel asks to choose while it is not signed in. Local.
#[cfg(feature = "pc")]
pub const HARNESS_CLAUDE_AUTH: SettingDefinition<String> = SettingDefinition {
    key: "harness_claude_auth",
    scope: SettingScope::Global,
    description: "Как платит Claude Code",
    sensitive: false,
    parse: |value| as_one_of(value, &["terminal", "key"]),
};

/// Where the programs of the agents' cards are, by the card's id, when not on the search path (stage 3б). Local.
#[cfg(feature = "pc")]
pub const HARNESS_PATHS: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "harness_paths",
    scope: SettingScope::Global,
    description: "Где программы агентов",
    sensitive: false,
    parse: as_string_record,
};

/// The home's character was chosen with a voice of its own (clean-path check 18.11, item 70): once that voice is up on
/// the Турбо, the centre switches its «Голос» to the computer's by itself — unless the owner chose a voice by hand
/// after. `{"character": <id>, "armed": "1"|"0"}`: armed when the home's character changes to one with a package;
/// disarmed by the switch or by the owner's own choice (src/models/home_voice.rs).
pub const HOME_VOICE_FOLLOW: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "home_voice_follow",
    scope: SettingScope::Global,
    description: "Голос персонажа включается сам, когда встал на Турбо",
    sensitive: false,
    parse: as_string_record,
};

/// The model the home's character speaks with on its Турбо, as the owner chose it at the centre (clean-path check
/// 18.11, item 40): by the character's id, or by its package's id when chosen before the package was installed. The
/// home's: it travels with the home.
pub const HOME_VOICE_ENGINES: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "home_voice_engines",
    scope: SettingScope::Global,
    description: "Модель голоса персонажа на Турбо",
    sensitive: false,
    parse: as_string_record,
};

/// On a Турбо: the model its centre named for a character's voice, by the character's id — the voice goes with it
/// instead of the one this computer would pick. The Турбо's own.
#[cfg(feature = "pc")]
pub const TURBO_VOICE_ENGINES: SettingDefinition<BTreeMap<String, String>> = SettingDefinition {
    key: "turbo_voice_engines",
    scope: SettingScope::Global,
    description: "Модель голоса персонажа, названная хабом",
    sensitive: false,
    parse: as_string_record,
};

/// On a Турбо: «Что этот компьютер даёт дому» (docs/plans/turbo-devices.md) — by the kind of a device (`midi`, `screen`,
/// `storage`, `folders`, `camera`, …), whether its home gets it. A kind not named here is given unless it is sensitive
/// (owner 20.11, 3: everything but the camera, the microphone and keyboard-like things). The computer's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurboShares {
    #[serde(default)]
    pub kinds: BTreeMap<String, bool>,
}

impl TurboShares {
    /// Whether the home gets this kind: as ticked, or by its default.
    pub fn gives(&self, kind: &str, sensitive: bool) -> bool {
        self.kinds.get(kind).copied().unwrap_or(!sensitive)
    }
}

pub const TURBO_SHARES: SettingDefinition<TurboShares> = SettingDefinition {
    key: "turbo_shares",
    scope: SettingScope::Global,
    description: "Что этот компьютер даёт дому",
    sensitive: false,
    parse: as_serde,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_connectors_with_their_defaults() {
        let sip = (CONNECTOR_SIP.parse)(&json!({"url": "http://127.0.0.1:8088/ari", "user": "duowave"})).unwrap();
        assert!(sip.enabled);
        assert_eq!((sip.app.as_str(), sip.rtp_host.as_str(), sip.rtp_port), ("duowave", "127.0.0.1", 40_000));
        assert_eq!((CONNECTOR_LIVEKIT.parse)(&json!({})).unwrap(), LiveKitSetting::default());
        let telegram = (CONNECTOR_TELEGRAM.parse)(
            &json!({"enabled": false, "contacts": [{"name": "мама", "to": "1"}], "alertChat": "1"}),
        )
        .unwrap();
        assert!(!telegram.enabled);
        assert_eq!(telegram.contacts, vec![ContactEntry { name: "мама".into(), to: "1".into() }]);
        assert_eq!(telegram.alert_chat.as_deref(), Some("1"));
        let news = (CONNECTOR_NEWS.parse)(&json!({"feeds": [{"url": "https://tass.ru/rss/v2.xml", "topic": "main"}]}))
            .unwrap();
        assert!(news.feeds[0].enabled);
        assert_eq!(
            serde_json::to_value(&news).unwrap(),
            json!({"feeds": [{"url": "https://tass.ru/rss/v2.xml", "topic": "main", "enabled": true}]})
        );
        assert_eq!((CONNECTOR_HOME_ASSISTANT.parse)(&json!("not an object")), None);
    }

    /// The home and the gateway are known by their ids: a record without one is none, and a name is optional.
    #[test]
    fn the_home_and_this_gateway_are_read_only_with_an_id() {
        let home = (HOME_IDENTITY.parse)(&json!({"id": "h1"})).unwrap();
        assert_eq!(home, HomeIdentity { id: "h1".into(), name: None });
        assert_eq!(serde_json::to_value(&home).unwrap(), json!({"id": "h1"}), "no name stored while nobody gave one");
        assert_eq!((HOME_IDENTITY.parse)(&json!({"id": " "})), None);
        assert_eq!((HOME_IDENTITY.parse)(&json!({"name": "Дом"})), None);
        let me = (GATEWAY_SELF.parse)(&json!({"id": "g1", "name": "Кухня"})).unwrap();
        assert_eq!(me, GatewaySelf { id: "g1".into(), name: Some("Кухня".into()) });
        assert_eq!((GATEWAY_SELF.parse)(&json!("g1")), None);
    }

    #[test]
    fn parses_what_it_stored_and_nothing_else() {
        assert_eq!((MODE.parse)(&json!({"mode": "own"})), Some(ModeSetting { mode: PanelMode::Own }));
        assert_eq!((MODE.parse)(&json!({"mode": "fly"})), None);
        assert_eq!(
            (VOICE_INPUT.parse)(&json!({"input": "computer", "before": "relay"})),
            Some(VoiceInput { input: TalkInput::Computer, before: Some(PanelMode::Relay) })
        );
        assert_eq!((VOICE_INPUT.parse)(&json!({"input": "station"})), Some(VoiceInput::default()));
        assert_eq!((VOICE_INPUT.parse)(&json!({"input": "phone"})), None);
        assert_eq!(
            serde_json::to_value(VoiceInput { input: TalkInput::Computer, before: Some(PanelMode::Gateway) }).unwrap(),
            json!({"input": "computer", "before": "gateway"})
        );
        assert_eq!((PERSONA.parse)(&json!({"presetId": null, "text": "  Ты — Алиса  "})).unwrap().text, "Ты — Алиса");
        assert_eq!((PERSONA.parse)(&json!({"text": " "})), None);
        assert_eq!((AUTH_SECRET.parse)(&json!("short")), None);
        let household =
            (HOUSEHOLD.parse)(&json!({"role": "nobody", "restrictions": {"deny": ["use:calls", 5]}})).unwrap();
        assert_eq!(household.role, Role::Guest);
        assert_eq!(household.restrictions.deny, Some(vec!["use:calls".to_string()]));
        let brain = (BRAIN.parse)(
            &json!({"fast": {"provider": "p", "model": "m"}, "smart": {"provider": "p", "model": ""}, "effort": "max"}),
        )
        .unwrap();
        assert_eq!(brain.fast, Some(BrainPick { provider: "p".into(), model: "m".into() }));
        assert_eq!(brain.smart, None);
        assert_eq!(brain.effort.as_deref(), Some("max"));
        assert_eq!((KEEP_REQUESTS.parse)(&json!(3651)), None);
        assert_eq!((KEEP_REQUESTS.parse)(&json!(-1)), None);
        let links = (STATION_LINKS.parse)(&json!({"192.168.1.5": {"fingerprint": "SHA256:x", "original": {"url": "wss://y", "quic": ""}}, "bad": {"x": 1}})).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            serde_json::to_value(&links["192.168.1.5"]).unwrap(),
            json!({"fingerprint": "SHA256:x", "original": {"url": "wss://y", "quic": ""}})
        );
    }
}
