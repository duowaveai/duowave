//! Model providers from the panel and the choice of models (src/dialog/llm/brain.rs): the brain the dialog asks per
//! answer. Rows and the choice are read at construction (the dialog says which model it uses at start) and after
//! every change; clients are built per provider when first needed and rebuilt when it or the key changes.
//!
//! The helpers' models (docs/plans/home-and-helpers.md, block 1, stage 4): each helper with a model has a row of its
//! own, «Модель: <helper>», made and changed with the helper. Who answers is decided per answer, knowing the helpers:
//! a chosen model whose helper is away gives way to the backup at once — no request goes to the helper —, else to the
//! next helper with a model, else to honest words in place of ten seconds of silence; the regulator «Что может
//! уходить из дома» keeps the clouds' models in. Where the main model is a helper's, the gateway sets a fast cloud
//! model as the backup by itself, when one is named exactly (default 6).

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use futures::StreamExt;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::helper_models::{HelperKind, HelperState, ModelHelper, ModelHelpers, Reach};
use super::presets::{
    headers_for, model_for, no_address, preset_of, suggested_of, ProviderKind, ProviderPreset, HELPER_NO_ADDRESS,
    HELPER_PRESET, PROVIDER_PRESETS,
};
use crate::config::{DialogConfig, DialogProvider, ModelPair, ModelProfile, LLM_EFFORTS};
use crate::core::availability::{State as Answering, CLOUDS_SHUT};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::dialog::llm::brain::{
    serialize_kind, Backup, BrainModels, BrainNow, BrainOrigin, BrainProfile, BrainProfiles, BrainSource, BrainStatus,
    EnvBrain, ModelSlot, ENV_PROVIDER, WORDS_PROVIDER,
};
use crate::dialog::llm::openai_chat::{tokens_field_for, OPENAI_BASE_URL};
use crate::dialog::llm::unreachable::Unreachable;
use crate::dialog::llm::{
    bearer_of, create_llm, describe_http_error as describe, Access, LlmClient, LlmEvent, LlmMessage, LlmRequest,
    LlmSettings, LlmStream, LlmTool, SpokenFailure,
};
use crate::oauth::{
    record_id, tokens, AccountView, Credential, Flow, OAuthService, SignInStart, Target, TokenAccess, TokenRecord,
};
use crate::operations::networks::is_local_url;
use crate::platform::http::ApiError;
use crate::secrets::{SecretMask, SecretsService};
use crate::settings::definitions::{BackupBy, Brain, BrainPick, BRAIN};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::http::client;
use crate::utils::time::now_iso;

/// A check's request may not wait longer: a spoken answer could not either.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Models of an OpenAI-compatible list that do not chat.
static NOT_CHAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(embed|whisper|tts|dall-e|moderation|transcri|image|audio|realtime|search|rerank)").unwrap()
});

fn key_of(id: &str) -> String {
    format!("llm:{id}:key")
}

/// A helper's row's stored title before the helper's name: «Модель: Компьютер дома» (the source language, what an
/// older panel shows; the panel's own is `tr!`'s).
const HELPER_TITLE: &str = "Модель: ";
/// Why another model answers, or none (`BrainNow.reason`; static, their English is in services.json): this computer's
/// model server away; the regulator's clouds shut is the registry's own word (`CLOUDS_SHUT`, core/availability.rs).
pub const BUILTIN_AWAY: &str = "модель на этом компьютере не отвечает";

/// The last «Проверить» of a provider, kept on its card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheck {
    pub at: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// From the request to the first word, ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_word_ms: Option<u64>,
    /// The model called a test tool when asked: it can use MCP and the station's commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    /// Why it failed, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A row of `llm_providers`.
#[derive(Debug, Clone, PartialEq)]
struct ProviderRow {
    id: String,
    preset: String,
    kind: ProviderKind,
    title: String,
    base_url: Option<String>,
    options: BTreeMap<String, String>,
    models: Vec<String>,
    last_check: Option<ProviderCheck>,
    /// Tells this provider from a later one that took its id.
    created_at: String,
    updated_at: String,
}

impl ProviderRow {
    /// The helper whose model this row is (the hidden preset `helper`).
    fn helper(&self) -> Option<&str> {
        (self.preset == HELPER_PRESET).then(|| self.options.get("helper").map(String::as_str)).flatten()
    }

    /// A helper's row's name of its helper as stored: kept while the helper is gone.
    fn helper_name(&self) -> &str {
        self.title.strip_prefix(HELPER_TITLE).unwrap_or(&self.title)
    }

    /// How far a helper's row's questions go as stored: home, or mine.
    fn stored_reach(&self) -> Reach {
        if self.options.get("reach").map(String::as_str) == Some("home") {
            Reach::Home
        } else {
            Reach::Mine
        }
    }
}

/// A provider as the panel shows it: never its key, only whether one is set.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub preset: String,
    pub kind: ProviderKind,
    pub title: String,
    pub base_url: Option<String>,
    pub options: BTreeMap<String, String>,
    pub key: SecretMask,
    pub needs_key: bool,
    pub editable_url: bool,
    pub models: Vec<String>,
    pub check: Option<ProviderCheck>,
    /// Its questions leave the house.
    pub cloud: bool,
    pub local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<&'static str>,
    /// A provider whose access is an account's (ChatGPT's plan): signed in or not, whose, what stands in the way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountView>,
    /// The helper whose model this is: a helper's row, or a model server of this computer (stage 6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<ProviderHelper>,
}

/// The helper behind a provider, for its mark in «Мозг».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderHelper {
    pub id: String,
    pub name: String,
    pub kind: HelperKind,
    /// `off` — no longer given: switched off, kept from the model, gone from the list.
    pub state: HelperState,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceView {
    pub fast: Option<BrainPick>,
    pub smart: Option<BrainPick>,
    pub effort: String,
    /// Answers when the chosen model gives no answer; `env` as the provider: the .env model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<BrainPick>,
    /// `gateway` — the gateway chose the backup by itself; `declined` — a person said «не нужна»; absent — a person's
    /// backup, or «пусть выберет шлюз» when there is none.
    #[serde(rename = "backupBy", skip_serializing_if = "Option::is_none")]
    pub backup_by: Option<BackupBy>,
    /// This gateway sets the backup by itself while nobody declined one, and takes `backup: "auto"` to hand the choice
    /// back to it («пусть выберет шлюз», owner's decision 18). A gateway without block «Дом» (0.13.0, 0.14.0) has
    /// neither: the panel then offers only «не нужна».
    #[serde(rename = "autoBackup")]
    pub auto_backup: bool,
}

/// `backup: "auto"` in a choice: «пусть выберет шлюз» — no backup of a person's and no «не нужна»: the gateway sets
/// one by itself when it may (default 6).
pub const AUTO_BACKUP: &str = "auto";

/// The model .env describes: it answers while the panel has chosen nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvView {
    #[serde(serialize_with = "serialize_kind")]
    pub provider: DialogProvider,
    pub title: String,
    pub models: BrainModels,
    pub usable: bool,
    pub cloud: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrainView {
    pub presets: Vec<&'static ProviderPreset>,
    pub providers: Vec<ProviderView>,
    pub choice: ChoiceView,
    pub status: BrainStatus,
    pub env: EnvView,
    /// One line about the backup while the main model is a helper's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<BrainNote>,
}

/// «Мозг»'s line about the backup: `backupAuto` (the gateway chose it, and it may cost money), `backupNeeded` (none
/// can be chosen by itself: choose one), `backupClosed` (the regulator keeps the clouds in: words instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrainNote {
    pub kind: &'static str,
    pub text: String,
}

/// A provider's fields from the panel, as JSON: absent is unchanged; for the key '' removes it.
#[derive(Debug, Clone, Default)]
pub struct ProviderInput(pub Map<String, Value>);

impl ProviderInput {
    pub fn from_value(value: Value) -> ProviderInput {
        match value {
            Value::Object(map) => ProviderInput(map),
            _ => ProviderInput::default(),
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}

#[derive(Default)]
struct State {
    rows: Vec<ProviderRow>,
    choice: Brain,
}

/// Whether «Выход в интернет» refuses a provider's calls now, by the provider's id (`BrainService::set_refusals`).
pub type Refusals = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A provider's client and the stamp it was built for (the row's change, the key's hint, the depth).
struct Built {
    stamp: String,
    client: Arc<dyn LlmClient>,
}

pub struct BrainService {
    config: DialogConfig,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    env: EnvBrain,
    state: RwLock<State>,
    /// The panel's changes one at a time: each reads the stored rows, writes a row and its key, and reloads as one
    /// step (the Node gateway could not interleave them). Who pairs a row with its key — a client, a check — holds
    /// it too, so a key never meets another provider's address.
    mutation: Mutex<()>,
    clients: Mutex<HashMap<String, Built>>,
    /// The access of each provider signed in with tokens, by id with its `created_at`: one per provider, so its
    /// clients and checks share one renewal at a time.
    accesses: Mutex<HashMap<String, (String, Arc<TokenAccess>)>>,
    /// The helpers that give a model (helper_models.rs). Unset — no helper is known and every road is open: the rows
    /// are asked as they are stored, as before the helpers.
    helpers: OnceLock<Arc<dyn ModelHelpers>>,
    /// Whether this gateway keeps its own settings: only then it sets the backup by itself (a station whose settings
    /// the computer keeps takes the computer's choice). Unset — it does.
    owner: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Whether «Выход в интернет» (src/egress) refuses a provider's calls now, by its id: its server of the person's
    /// does not answer, or it waits for a connection in a country its service works from. Such a model stands in for
    /// nobody — not in an away helper's place, not as the gateway's own backup —: it would only say «VPN не отвечает».
    /// Unset — none is refused, as before the routes.
    refusals: OnceLock<Refusals>,
    /// The rows whose model list this start asked for by itself (by id and `created_at`), listed or being asked: once
    /// each; one whose request failed is let go of, to be asked again.
    listed: Mutex<HashSet<(String, String)>>,
}

/// The helpers as «Мозг» read them for one decision — before its own lock (helper_models.rs).
#[derive(Default)]
struct Seen {
    /// The helpers are known (`set_helpers`): without them every road is open and no helper is known.
    known: bool,
    helpers: Vec<ModelHelper>,
    /// The regulator: home, mine, foreign.
    lets: [bool; 3],
}

impl Seen {
    fn lets(&self, reach: Reach) -> bool {
        !self.known
            || self.lets[match reach {
                Reach::Home => 0,
                Reach::Mine => 1,
                Reach::Foreign => 2,
            }]
    }

    /// The helper of a helper's row.
    fn helper(&self, id: &str) -> Option<&ModelHelper> {
        self.helpers.iter().find(|helper| helper.id == id && helper.provider.is_none())
    }

    /// The model helper a row already is: a model server of this computer (stage 6).
    fn named(&self, row: &str) -> Option<&ModelHelper> {
        self.helpers.iter().find(|helper| helper.provider.as_deref() == Some(row))
    }
}

/// A string field as the panel sent it, trimmed and cut to `max` characters; '' for anything else.
fn text(value: Option<&Value>, max: usize) -> String {
    let Some(text) = value.and_then(Value::as_str) else { return String::new() };
    let mut out = String::new();
    let mut units = 0;
    for c in text.trim().chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

/// The first `max` characters.
fn head(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// About how `localeCompare` ordered model ids: punctuation, then digits, then letters in any case, lower case first
/// between equals.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    const PUNCTUATION: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    fn primary(c: char) -> (u8, u32) {
        if let Some(rank) = PUNCTUATION.find(c) {
            (0, rank as u32)
        } else if c.is_ascii_digit() {
            (1, c as u32)
        } else if c.is_alphabetic() {
            (2, c.to_lowercase().next().unwrap_or(c) as u32)
        } else {
            (3, c as u32)
        }
    }
    let primaries = |text: &str| text.chars().map(primary).collect::<Vec<_>>();
    primaries(a).cmp(&primaries(b)).then_with(|| {
        let cases = |text: &str| text.chars().map(|c| c.is_uppercase()).collect::<Vec<_>>();
        cases(a).cmp(&cases(b)).then_with(|| a.cmp(b))
    })
}

static NO_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)no API key").unwrap());
static KEY_REFUSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(401|403)\b|rejected the API key|unauthori[sz]ed|invalid.*key|permission").unwrap()
});
static NOT_FOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b404\b|does not know the model|model.*not.*(found|exist)").unwrap());
static REFUSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(402|429)\b|rate limit|quota|insufficient|billing|credit").unwrap());
static UNREACHABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)ECONNREFUSED|ENOTFOUND|EAI_AGAIN|fetch failed|cannot reach|ECONNRESET|connection refused|dns error|error sending request|нет связи с",
    )
    .unwrap()
});
static TIMED_OUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)timeout|timed out|aborted|TimeoutError|did not answer in time|не ответил вовремя").unwrap()
});

/// Reads a stream to its end, cancelling it after the check's time.
async fn drain(
    mut stream: LlmStream,
    cancel: CancellationToken,
    mut on_event: impl FnMut(&LlmEvent),
) -> anyhow::Result<()> {
    let timer = tokio::time::sleep(CHECK_TIMEOUT);
    tokio::pin!(timer);
    loop {
        tokio::select! {
            _ = &mut timer, if !cancel.is_cancelled() => cancel.cancel(),
            item = stream.next() => match item {
                None => return Ok(()),
                Some(Ok(event)) => on_event(&event),
                Some(Err(error)) => return Err(error),
            },
        }
    }
}

impl BrainService {
    /// The service with the stored providers and choice; a database that cannot give them stops the start, as the
    /// Node gateway's constructor threw.
    pub fn new(
        config: DialogConfig,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
    ) -> Result<BrainService, ApiError> {
        let env = EnvBrain::new(LlmSettings::from_dialog(&config));
        let service = BrainService {
            config,
            db,
            settings,
            secrets,
            events,
            env,
            state: RwLock::new(State::default()),
            mutation: Mutex::new(()),
            clients: Mutex::new(HashMap::new()),
            accesses: Mutex::new(HashMap::new()),
            helpers: OnceLock::new(),
            owner: OnceLock::new(),
            refusals: OnceLock::new(),
            listed: Mutex::new(HashSet::new()),
        };
        service.fill_no_address()?;
        service.reload()?;
        Ok(service)
    }

    /// The helpers that give a model: from now on each has its row, and who answers knows whether they are on the
    /// line.
    pub fn set_helpers(&self, helpers: Arc<dyn ModelHelpers>) {
        let _ = self.helpers.set(helpers);
    }

    /// Whether this gateway keeps its own settings now (placement's `owns_settings`).
    pub fn set_owner(&self, owns: Arc<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.owner.set(owns);
    }

    fn owns(&self) -> bool {
        self.owner.get().is_none_or(|owns| owns())
    }

    /// What «Выход в интернет» refuses now (app.rs: its published decisions, `egress::refused_row`).
    pub fn set_refusals(&self, refused: Refusals) {
        let _ = self.refusals.set(refused);
    }

    /// Whether a provider's calls are refused now: its model stands in for nobody.
    fn refused(&self, provider: &str) -> bool {
        self.refusals.get().is_some_and(|refused| refused(provider))
    }

    /// The helpers now, read before `mutation` is taken.
    fn seen(&self) -> Seen {
        let Some(helpers) = self.helpers.get() else { return Seen::default() };
        let lets = [Reach::Home, Reach::Mine, Reach::Foreign].map(|reach| helpers.lets(reach));
        Seen { known: true, helpers: helpers.model_helpers(), lets }
    }

    // The panel.

    pub fn view(&self) -> BrainView {
        let seen = self.seen();
        let (rows, choice) = self.snapshot();
        // A helper's model is the gateway's own row: never offered.
        let mut presets: Vec<&'static ProviderPreset> =
            PROVIDER_PRESETS.iter().filter(|preset| preset.id != HELPER_PRESET).collect();
        presets.sort_by_key(|preset| preset.order);
        let env_status = self.env.status();
        BrainView {
            presets,
            providers: rows.iter().map(|row| self.provider_view(row, &seen)).collect(),
            choice: ChoiceView {
                fast: choice.fast.clone(),
                smart: choice.smart.clone(),
                effort: self.effort_of(&choice),
                backup: choice.backup.clone(),
                backup_by: choice.backup_by,
                auto_backup: true,
            },
            note: self.note(&rows, &choice, &seen),
            status: self.status_of(&rows, &choice),
            env: EnvView {
                provider: self.config.provider,
                title: env_status
                    .profiles
                    .fast
                    .as_ref()
                    .map(|profile| profile.title.clone())
                    .unwrap_or_else(|| crate::tr!("нет")),
                models: BrainModels { fast: self.config.models.fast.clone(), smart: self.config.models.smart.clone() },
                usable: self.env.slot(ModelProfile::Fast).is_some(),
                cloud: env_status.cloud,
            },
        }
    }

    pub fn add(&self, input: &ProviderInput, actor: Option<Actor>) -> Result<ProviderView, ApiError> {
        let preset = input
            .get("preset")
            .and_then(Value::as_str)
            .and_then(preset_of)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("Неизвестный провайдер.")))?;
        if preset.id == HELPER_PRESET {
            return Err(ApiError::bad_request(crate::tr!("Модель Турбо появляется здесь сама.")));
        }
        let base_url = address(preset, input.get("baseUrl"))?;
        let title = Some(text(input.get("title"), 60)).filter(|title| !title.is_empty()).unwrap_or(preset.title.into());
        let options = fields(preset, input.get("options"));
        let seen = self.seen();
        let (id, view) = {
            let _mutation = self.lock();
            let id = new_id(&self.ids()?, preset.id);
            // A key or a sign-in left under this id by an earlier provider (a removal that failed half-way) never
            // reaches this one.
            self.secrets.delete_all(&format!("llm:{id}:"))?;
            self.secrets.delete(&record_id(&id))?;
            let now = now_iso();
            self.db.with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, models, last_check,
                     created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '[]', NULL, ?7, ?7)",
                    params![id, preset.id, preset.kind.as_str(), title, base_url, json!(options).to_string(), now],
                )
            })?;
            if let Some(key) = input.get("key").and_then(Value::as_str) {
                self.secrets.set(&key_of(&id), Some(key.trim()))?;
            }
            self.reload()?;
            let view = self.provider_view(&self.row(&id)?, &seen);
            (id, view)
        };
        self.events.emit(GatewayEvent::BrainChanged { change: "provider.added".into(), provider_id: Some(id), actor });
        Ok(view)
    }

    /// Changes what the input names, nothing else; the key only of a provider that is still there.
    pub fn update(&self, id: &str, input: &ProviderInput, actor: Option<Actor>) -> Result<ProviderView, ApiError> {
        let seen = self.seen();
        let view = {
            let _mutation = self.lock();
            let row = self.stored_row(id)?.ok_or_else(missing)?;
            if row.helper().is_some() {
                return Err(helpers_own());
            }
            let preset = preset_of(&row.preset);
            let title = input.get("title").map(|value| {
                Some(text(Some(value), 60))
                    .filter(|title| !title.is_empty())
                    .or_else(|| preset.map(|preset| preset.title.to_string()))
                    .unwrap_or_else(|| row.title.clone())
            });
            let base_url = match (preset, input.get("baseUrl")) {
                (Some(preset), Some(value)) => Some(address(preset, Some(value))?),
                _ => None,
            };
            let options = match (preset, input.get("options")) {
                (Some(preset), Some(value)) => Some(json!(fields(preset, Some(value))).to_string()),
                _ => None,
            };
            let changed = self.db.with(|c| {
                c.execute(
                    "UPDATE llm_providers SET title = COALESCE(?2, title),
                     base_url = CASE WHEN ?3 THEN ?4 ELSE base_url END, options = COALESCE(?5, options),
                     updated_at = ?6 WHERE id = ?1",
                    params![id, title, base_url.is_some(), base_url.flatten(), options, now_iso()],
                )
            })?;
            // Gone meanwhile: no key is written, so none waits under an id the next provider may get.
            if changed == 0 {
                return Err(missing());
            }
            if let Some(key) = input.get("key").and_then(Value::as_str) {
                self.secrets.set(&key_of(id), Some(key.trim()))?;
            }
            self.reload()?;
            self.provider_view(&self.row(id)?, &seen)
        };
        self.events.emit(GatewayEvent::BrainChanged {
            change: "provider.updated".into(),
            provider_id: Some(id.to_string()),
            actor,
        });
        Ok(view)
    }

    /// A provider gone, with its key and its sign-in. A helper's row goes with its helper; once its helper is no longer
    /// listed, a row a choice kept is the person's to remove like any other.
    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let seen = self.seen();
        {
            let _mutation = self.lock();
            if !self.ids()?.iter().any(|known| known == id) {
                return Err(missing());
            }
            let listed = |helper: &str| !seen.known || seen.helper(helper).is_some();
            if self.stored_row(id)?.is_some_and(|row| row.helper().is_some_and(listed)) {
                return Err(helpers_own());
            }
            let choice = self.settings.peek(&BRAIN)?.unwrap_or_default();
            let answering = |pick: &Option<BrainPick>| pick.as_ref().is_some_and(|pick| pick.provider == id);
            if answering(&choice.fast) || answering(&choice.smart) {
                return Err(ApiError::conflict(crate::tr!(
                    "Этот провайдер сейчас отвечает. Сначала выберите другую модель."
                )));
            }
            // The gateway's own pick is no person's choice to keep: it goes with its provider, and after the removal's
            // news the gateway looks again among the clouds left (keep_backup). A person's backup stays theirs to change.
            let gateways_pick = answering(&choice.backup) && choice.backup_by == Some(BackupBy::Gateway);
            if answering(&choice.backup) && !gateways_pick {
                return Err(ApiError::conflict(crate::tr!(
                    "Этот провайдер — запасная модель. Сначала выберите в «Запасной» другую модель, «пусть выберет центр» или «не нужна»."
                )));
            }
            let flow = self.stored_row(id)?.and_then(|row| token_flow(&row.preset));
            if self.db.with(|c| c.execute("DELETE FROM llm_providers WHERE id = ?1", params![id]))? == 0 {
                return Err(missing());
            }
            if gateways_pick {
                let mut next = choice;
                next.backup = None;
                next.backup_by = None;
                self.settings.set(&BRAIN, &next, actor.clone())?;
            }
            self.secrets.delete_all(&format!("llm:{id}:"))?;
            let signed: Option<TokenRecord> =
                self.secrets.get(&record_id(id)).and_then(|text| serde_json::from_str(&text).ok());
            self.secrets.delete(&record_id(id))?;
            self.accesses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
            // The sign-in ends at the service too; unconfirmed, the owner disconnects DuoWave in the service's settings.
            if let (Some(flow), Some(record), Ok(runtime)) = (flow, signed, tokio::runtime::Handle::try_current()) {
                let provider = id.to_string();
                runtime.spawn(async move {
                    if !tokens::revoke(flow, &record).await {
                        tracing::warn!(target: "brain", "{provider}: the sign-in was not revoked at the service");
                    }
                });
            }
            self.reload()?;
        }
        self.events.emit(GatewayEvent::BrainChanged {
            change: "provider.removed".into(),
            provider_id: Some(id.to_string()),
            actor,
        });
        Ok(())
    }

    /// «Войти через …»: the sign-in of a provider whose service has one (src/oauth). What it brings lands on this
    /// provider as it is now — not on a later one that takes its id meanwhile. `here`: the panel is open on this
    /// computer, so the browser's return reaches the gateway.
    pub fn sign_in(&self, id: &str, engine: &OAuthService, here: bool) -> Result<SignInStart, ApiError> {
        let row = {
            let _mutation = self.lock();
            self.stored_row(id)?.ok_or_else(missing)?
        };
        let sign_in = preset_of(&row.preset)
            .and_then(|preset| preset.sign_in.as_ref())
            .ok_or_else(|| ApiError::bad_request(crate::tr!("Этот провайдер подключается только ключом.")))?;
        engine.start(sign_in, target_of(&row), here)
    }

    /// What a sign-in brought, on the provider that began it — still there and not replaced by another under its id:
    /// a key kept as a pasted one would be, or tokens kept on this gateway. The provider's title for the panel.
    pub fn signed_in(
        &self,
        id: &str,
        created_at: &str,
        credential: Credential,
        actor: Option<Actor>,
    ) -> Result<String, ApiError> {
        let title = {
            let _mutation = self.lock();
            let row = self.stored_row(id)?.filter(|row| row.created_at == created_at).ok_or_else(missing)?;
            match credential {
                Credential::Key(key) => self.secrets.set(&key_of(id), Some(&key))?,
                Credential::Tokens(record) => {
                    let text = serde_json::to_string(&record).map_err(ApiError::internal)?;
                    self.secrets.set(&record_id(id), Some(&text))?;
                    if let Some(access) = self.access_of(&row) {
                        access.reset();
                    }
                }
            }
            self.db.with(|c| {
                c.execute(
                    "UPDATE llm_providers SET updated_at = ?3 WHERE id = ?1 AND created_at = ?2",
                    params![id, created_at, now_iso()],
                )
            })?;
            self.reload()?;
            row.title
        };
        self.events.emit(GatewayEvent::BrainChanged {
            change: "provider.login".into(),
            provider_id: Some(id.to_string()),
            actor,
        });
        Ok(title)
    }

    /// Who gets what a provider's sign-in brings: for tests that sign in with a service of their own.
    #[cfg(test)]
    pub(crate) fn sign_in_target(&self, id: &str) -> Result<Target, ApiError> {
        let _mutation = self.lock();
        self.stored_row(id)?.map(|row| target_of(&row)).ok_or_else(missing)
    }

    /// The models: fast and smart (null: back to .env; a smart left out follows the fast one), the backup (null: none,
    /// «не нужна» — the gateway sets none by itself; `"auto"`: the gateway's to choose; `env` as the provider: the .env
    /// model) and Claude's depth.
    pub fn choose(&self, input: &Map<String, Value>, actor: Option<Actor>) -> Result<BrainView, ApiError> {
        let mutation = self.lock();
        let rows = self.load_rows()?;
        let mut next = self.settings.peek(&BRAIN)?.unwrap_or_default();
        for profile in ["fast", "smart", "backup"] {
            let Some(value) = input.get(profile) else { continue };
            // «Пусть выберет шлюз» (owner's decision 18): a person's backup, a «не нужна» and the gateway's own pick
            // all go. After this choice's news the gateway looks again (keep_backup) and takes what qualifies now: the
            // same model while it still does, another when its key went or VPN refuses it, none beside a cloud main.
            if profile == "backup" && value.as_str() == Some(AUTO_BACKUP) {
                next.backup = None;
                next.backup_by = None;
                continue;
            }
            let pick = if value.is_null() {
                None
            } else {
                let provider = value.get("provider").and_then(Value::as_str);
                // The .env model may be the backup when it can answer.
                let env = profile == "backup" && self.env.slot(ModelProfile::Fast).is_some();
                let known =
                    |provider: &&str| rows.iter().any(|row| row.id == *provider) || (env && *provider == ENV_PROVIDER);
                let Some(provider) = provider.filter(known) else {
                    return Err(ApiError::bad_request(crate::tr!("Такого провайдера нет.")));
                };
                let model = text(value.get("model"), 200);
                if model.is_empty() {
                    return Err(ApiError::bad_request(crate::tr!("Выберите модель.")));
                }
                Some(BrainPick { provider: provider.to_string(), model })
            };
            match profile {
                "fast" => next.fast = pick,
                "smart" => next.smart = pick,
                _ => {
                    // A person's choice is theirs; their «не нужна» is remembered: the gateway sets none by itself.
                    next.backup_by = if pick.is_some() { None } else { Some(BackupBy::Declined) };
                    next.backup = pick;
                }
            }
        }
        if let Some(effort) = input.get("effort") {
            match effort.as_str().filter(|effort| LLM_EFFORTS.contains(effort)) {
                Some(effort) => next.effort = Some(effort.to_string()),
                None => {
                    return Err(ApiError::bad_request(crate::tr!(
                        "Глубина мышления: low, medium, high, xhigh или max."
                    )))
                }
            }
        }
        if next.fast.is_none() {
            next.smart = None;
        }
        self.settings.set(&BRAIN, &next, actor.clone())?;
        self.reload()?;
        drop(mutation);
        self.events.emit(GatewayEvent::BrainChanged { change: "choice".into(), provider_id: None, actor });
        tracing::info!(target: "brain", "dialog: {}", self.description());
        Ok(self.view())
    }

    /// The service's models, asked now (the panel's «Обновить»); also kept for the card. An unknown provider is an
    /// ApiError inside; a service that fails is a plain error (the route answers 500, as Nest did).
    pub async fn models(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let (row, settings) = self.paired(id)?;
        let models = match self.list_models(&row, &settings).await {
            Ok(models) => models,
            // Ollama or LM Studio not running or not installed: an everyday state the first steps look for, not a
            // fault — said in words, the reason in the log at info.
            Err(error) if preset_of(&row.preset).is_some_and(|preset| preset.local) => {
                tracing::info!(target: "brain", "models of {id}: {}", self.secrets.redact(&format!("{error:#}")));
                return Err(ApiError::bad_gateway(self.problem_of(&error)).into());
            }
            Err(error) => return Err(error),
        };
        let _mutation = self.lock();
        // Kept only on the provider that was asked: one that took its id since keeps its own list.
        self.db
            .with(|c| {
                c.execute(
                    "UPDATE llm_providers SET models = ?3 WHERE id = ?1 AND created_at = ?2",
                    params![id, row.created_at, json!(models).to_string()],
                )
            })
            .map_err(ApiError::from)?;
        self.reload()?;
        Ok(models)
    }

    /// «Проверить»: the list of models, a short answer with the time to the first word, and whether the model calls a
    /// tool when asked. The result stays on the card.
    pub async fn check(&self, id: &str, model: Option<&Value>) -> Result<ProviderCheck, ApiError> {
        // The row and its key as they are now, together: a provider that takes the id during the check gets
        // neither this check nor its key sent to this address.
        let (row, settings) = self.paired(id)?;
        let preset = preset_of(&row.preset);
        let at = now_iso();
        let mut models = row.models.clone();
        let mut list_problem = None;
        match self.list_models(&row, &settings).await {
            Ok(listed) => models = listed,
            Err(error) => list_problem = Some(self.problem_of(&error)),
        }
        let (_, choice) = self.snapshot();
        let given = text(model, 200);
        let chosen = [&choice.fast, &choice.smart].into_iter().flatten().find(|pick| pick.provider == id);
        let target = Some(given)
            .filter(|model| !model.is_empty())
            .or_else(|| chosen.map(|pick| pick.model.clone()).filter(|model| !model.is_empty()))
            .or_else(|| {
                preset
                    .and_then(|preset| preset.suggested.as_ref())
                    .map(|s| s.fast.to_string())
                    .filter(|m| !m.is_empty())
            })
            .or_else(|| models.first().cloned().filter(|model| !model.is_empty()));
        let Some(target) = target else {
            let problem =
                list_problem.unwrap_or_else(|| crate::tr!("Сервис не назвал ни одной модели: впишите её вручную."));
            return self.save_check(
                &row,
                &models,
                ProviderCheck { at, ok: false, model: None, first_word_ms: None, tools: None, problem: Some(problem) },
            );
        };
        let client = build(&settings);
        let full = model_for(&row.preset, &row.options, &target);
        let started = Instant::now();
        let mut first_word_ms = None;
        let words = LlmRequest {
            system: "Отвечай одним словом.".into(),
            messages: vec![LlmMessage::User("Скажи «да».".into())],
            model: Some(full.clone()),
            ..LlmRequest::default()
        };
        let cancel = CancellationToken::new();
        let answered = drain(client.stream(words, cancel.clone()), cancel, |event| {
            if matches!(event, LlmEvent::Text(_)) && first_word_ms.is_none() {
                first_word_ms = Some(started.elapsed().as_millis() as u64);
            }
        })
        .await;
        if let Err(error) = answered {
            let problem = Some(self.problem_of(&error));
            return self.save_check(
                &row,
                &models,
                ProviderCheck { at, ok: false, model: Some(target), first_word_ms: None, tools: None, problem },
            );
        }
        let mut tools = false;
        let probe = LlmRequest {
            system: "Ты проверяешь, работают ли инструменты.".into(),
            messages: vec![LlmMessage::User("Вызови инструмент check_tools.".into())],
            model: Some(full),
            tools: vec![LlmTool {
                name: "check_tools".into(),
                description: "Call this tool to confirm that tools work.".into(),
                parameters: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            }],
            ..LlmRequest::default()
        };
        let cancel = CancellationToken::new();
        if let Err(error) = drain(client.stream(probe, cancel.clone()), cancel, |event| {
            if matches!(event, LlmEvent::Tool(_)) {
                tools = true;
            }
        })
        .await
        {
            tracing::debug!(target: "brain", "{}: no tools: {}", row.title, self.secrets.redact(&error.to_string()));
        }
        self.save_check(
            &row,
            &models,
            ProviderCheck { at, ok: true, model: Some(target), first_word_ms, tools: Some(tools), problem: None },
        )
    }

    // Inside.

    /// The panel's changes one at a time (see `mutation`).
    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A helper's row stored without an address (by a build of the helpers before `HELPER_NO_ADDRESS`, or a package
    /// from one) takes the stand-in at the start: a version from before the helpers, taken back to, reads none as
    /// OpenAI's.
    fn fill_no_address(&self) -> Result<(), ApiError> {
        self.db.with(|c| {
            c.execute(
                "UPDATE llm_providers SET base_url = ?2 WHERE preset = ?1 AND (base_url IS NULL OR trim(base_url) = '')",
                params![HELPER_PRESET, HELPER_NO_ADDRESS],
            )
        })?;
        Ok(())
    }

    /// The stored rows and choice, in place of the ones held. One that cannot be read leaves the held ones as they
    /// are and is the caller's error (a 500), never an empty list: the dialog would quietly fall back to .env.
    /// Callers hold `mutation` (the constructor need not), so reloads never overtake one another.
    fn reload(&self) -> Result<(), ApiError> {
        let rows = self.load_rows()?;
        let choice = self.settings.peek(&BRAIN)?.unwrap_or_default();
        let mut state = self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = State { rows, choice };
        Ok(())
    }

    fn load_rows(&self) -> rusqlite::Result<Vec<ProviderRow>> {
        type Raw = (String, String, String, String, Option<String>, String, String, Option<String>, String, String);
        let raw: Vec<Raw> = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, preset, kind, title, base_url, options, models, last_check, created_at, updated_at
                 FROM llm_providers ORDER BY rowid",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            })?;
            rows.collect()
        })?;
        Ok(raw
            .into_iter()
            .filter_map(|(id, preset, kind, title, base_url, options, models, last_check, created_at, updated_at)| {
                let Some(kind) = ProviderKind::parse(&kind) else {
                    tracing::warn!(target: "brain", "provider {id}: unknown kind {kind}, skipped");
                    return None;
                };
                // The stand-in for no address (`HELPER_NO_ADDRESS`) is none here.
                let base_url = base_url.filter(|url| !(preset == HELPER_PRESET && no_address(url)));
                Some(ProviderRow {
                    id,
                    preset,
                    kind,
                    title,
                    base_url,
                    options: serde_json::from_str(&options).unwrap_or_default(),
                    models: serde_json::from_str(&models).unwrap_or_default(),
                    last_check: last_check.and_then(|check| serde_json::from_str(&check).ok()),
                    created_at,
                    updated_at,
                })
            })
            .collect())
    }

    /// Every id the table holds, a row of a kind this version does not know too: what a new provider must not take.
    fn ids(&self) -> rusqlite::Result<Vec<String>> {
        self.db.with(|c| {
            let mut statement = c.prepare("SELECT id FROM llm_providers")?;
            let ids = statement.query_map([], |row| row.get(0))?;
            ids.collect()
        })
    }

    /// The row as stored now (not as held).
    fn stored_row(&self, id: &str) -> rusqlite::Result<Option<ProviderRow>> {
        Ok(self.load_rows()?.into_iter().find(|row| row.id == id))
    }

    /// A provider's row and its settings with its key, read together under `mutation`: they belong to one provider. A
    /// helper's row is asked only as its helper may be: not once it is gone, nor past the regulator.
    fn paired(&self, id: &str) -> Result<(ProviderRow, LlmSettings), ApiError> {
        let seen = self.seen();
        let _mutation = self.lock();
        let row = self.stored_row(id)?.ok_or_else(missing)?;
        if let Some(helper) = row.helper().filter(|_| seen.known) {
            match seen.helper(helper) {
                None => {
                    return Err(ApiError::conflict(crate::tr!("{name} не на связи", name = row.helper_name())));
                }
                Some(helper) if !seen.lets(helper.reach) => {
                    return Err(ApiError::conflict(crate::tr!(
                        "Регулятор «Что может уходить из дома» не пускает этого Турбо."
                    )));
                }
                Some(_) => {}
            }
        }
        let settings = self.settings_of(&row, &seen);
        Ok((row, settings))
    }

    fn snapshot(&self) -> (Vec<ProviderRow>, Brain) {
        let state = self.state.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        (state.rows.clone(), state.choice.clone())
    }

    fn effort_of(&self, choice: &Brain) -> String {
        choice.effort.clone().unwrap_or_else(|| self.config.effort.clone())
    }

    fn effort(&self) -> String {
        self.effort_of(&self.snapshot().1)
    }

    fn picked<'a>(
        rows: &'a [ProviderRow],
        choice: &'a Brain,
        profile: ModelProfile,
    ) -> Option<(&'a ProviderRow, &'a BrainPick)> {
        let fast = choice.fast.as_ref()?;
        let pick = match profile {
            ModelProfile::Smart => choice.smart.as_ref().unwrap_or(fast),
            ModelProfile::Fast => fast,
        };
        rows.iter().find(|row| row.id == pick.provider).map(|row| (row, pick))
    }

    fn row(&self, id: &str) -> Result<ProviderRow, ApiError> {
        let (rows, _) = self.snapshot();
        rows.into_iter().find(|row| row.id == id).ok_or_else(missing)
    }

    fn provider_view(&self, row: &ProviderRow, seen: &Seen) -> ProviderView {
        let preset = preset_of(&row.preset);
        let cloud = cloud_now(row, seen);
        let helper = match row.helper() {
            Some(id) => Some(match seen.helper(id) {
                Some(helper) => ProviderHelper {
                    id: id.to_string(),
                    name: helper.name.clone(),
                    kind: helper.kind,
                    state: helper.state,
                    builtin: false,
                },
                None => ProviderHelper {
                    id: id.to_string(),
                    name: row.helper_name().to_string(),
                    kind: if row.stored_reach() == Reach::Home { HelperKind::Home } else { HelperKind::Own },
                    state: HelperState::Off,
                    builtin: false,
                },
            }),
            None => seen.named(&row.id).map(|helper| ProviderHelper {
                id: helper.id.clone(),
                name: helper.name.clone(),
                kind: helper.kind,
                state: helper.state,
                builtin: helper.builtin,
            }),
        };
        ProviderView {
            id: row.id.clone(),
            preset: row.preset.clone(),
            kind: row.kind,
            title: title_of(row, seen),
            base_url: row.base_url.clone(),
            options: row.options.clone(),
            key: self.secrets.mask(&key_of(&row.id)),
            needs_key: preset.is_some_and(|preset| preset.needs_key),
            editable_url: preset.is_none_or(|preset| preset.editable_url),
            models: row.models.clone(),
            check: row.last_check.clone(),
            cloud,
            local: !cloud,
            brand: preset.and_then(|preset| preset.brand),
            account: self.access_of(row).map(|access| access.account()),
            helper,
        }
    }

    /// The access of a provider signed in with tokens (ChatGPT's plan): one per provider, made when first needed.
    fn access_of(&self, row: &ProviderRow) -> Option<Arc<TokenAccess>> {
        let preset = preset_of(&row.preset)?;
        let sign_in = preset.sign_in.as_ref()?;
        let Flow::Tokens(flow) = sign_in.flow else { return None };
        let mut accesses = self.accesses.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, access)) = accesses.get(&row.id).filter(|(created_at, _)| *created_at == row.created_at) {
            return Some(access.clone());
        }
        let access = Arc::new(TokenAccess::new(self.secrets.clone(), &record_id(&row.id), flow, sign_in.with));
        accesses.insert(row.id.clone(), (row.created_at.clone(), access.clone()));
        Some(access)
    }

    /// A helper's row asks where its helper's model is now, with what its helper takes now (a plain entry's key, a
    /// linked entry's bridge token); a helper no longer listed is asked nowhere. Other rows: as stored.
    fn settings_of(&self, row: &ProviderRow, seen: &Seen) -> LlmSettings {
        let preset = preset_of(&row.preset);
        let (base_url, api_key) = match row.helper().filter(|_| seen.known) {
            Some(id) => match seen.helper(id) {
                Some(helper) => (Some(helper.base_url.clone()), helper.key.clone()),
                None => (None, None),
            },
            None => (
                row.base_url.clone().or_else(|| preset.and_then(|preset| preset.base_url).map(str::to_string)),
                self.secrets.get(&key_of(&row.id)),
            ),
        };
        // A helper's row with no address now is asked nowhere: at the stand-in, a closed port here — never at OpenAI's,
        // where an empty address sends a question.
        let base_url = base_url.or_else(|| (row.preset == HELPER_PRESET).then(|| HELPER_NO_ADDRESS.to_string()));
        LlmSettings {
            provider: row.kind.provider(),
            model: String::new(),
            models: ModelPair { fast: String::new(), smart: String::new() },
            tokens_field: tokens_field_for(base_url.as_deref().unwrap_or(OPENAI_BASE_URL)),
            base_url,
            api_key,
            max_tokens: self.config.max_tokens,
            effort: self.effort(),
            explicit: true,
            headers: headers_for(&row.preset, &row.options),
            access: self.access_of(row).map(|access| Access(access)),
            api: preset.map(|preset| preset.api).unwrap_or_default(),
        }
    }

    /// A provider's model as one answer's slot, without a backup. The caller holds `mutation`.
    fn slot_of_row(&self, row: &ProviderRow, model: &str, seen: &Seen) -> ModelSlot {
        ModelSlot {
            client: self.client_of(row, seen),
            model: model_for(&row.preset, &row.options, model),
            provider: row.id.clone(),
            kind: row.kind.provider(),
            title: title_of(row, seen),
            cloud: cloud_now(row, seen),
            backup: None,
        }
    }

    /// The provider's client, rebuilt when the row (or the provider under its id), its key or the thinking depth
    /// changed — a helper's row's also when its helper's address or key did. The caller holds `mutation`: the row and
    /// the key it reads belong to one provider.
    fn client_of(&self, row: &ProviderRow, seen: &Seen) -> Arc<dyn LlmClient> {
        let reached = match row.helper().and_then(|id| seen.helper(id)) {
            Some(helper) => format!("|{}|{}", helper.base_url, helper.key.as_deref().unwrap_or_default()),
            None => String::new(),
        };
        let stamp = format!(
            "{}|{}|{}|{}{reached}",
            row.created_at,
            row.updated_at,
            self.secrets.mask(&key_of(&row.id)).hint.unwrap_or_default(),
            self.effort()
        );
        let mut clients = self.clients.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(built) = clients.get(&row.id).filter(|built| built.stamp == stamp) {
            return built.client.clone();
        }
        let client = build(&self.settings_of(row, seen));
        clients.insert(row.id.clone(), Built { stamp, client: client.clone() });
        client
    }

    fn status_of(&self, rows: &[ProviderRow], choice: &Brain) -> BrainStatus {
        let Some(fast) = Self::picked(rows, choice, ModelProfile::Fast) else { return self.env.status() };
        let smart = Self::picked(rows, choice, ModelProfile::Smart).unwrap_or(fast);
        let profile = |(row, pick): (&ProviderRow, &BrainPick)| BrainProfile {
            provider: row.id.clone(),
            kind: row.kind.provider(),
            title: title_of(row, &Seen::default()),
            model: pick.model.clone(),
            cloud: cloud_of_row(row),
        };
        let claude = fast.0.kind == ProviderKind::Anthropic || smart.0.kind == ProviderKind::Anthropic;
        BrainStatus {
            provider: fast.0.kind.provider(),
            models: BrainModels { fast: fast.1.model.clone(), smart: smart.1.model.clone() },
            effort: claude.then(|| self.effort_of(choice)),
            cloud: cloud_of_row(fast.0),
            profiles: BrainProfiles { fast: Some(profile(fast)), smart: Some(profile(smart)) },
            source: BrainOrigin::Panel,
        }
    }

    /// The service's chat models, asked with the settings `paired` read for this row.
    async fn list_models(&self, row: &ProviderRow, settings: &LlmSettings) -> anyhow::Result<Vec<String>> {
        let preset = preset_of(&row.preset);
        let base = settings.base_url.as_deref().unwrap_or("").trim_end_matches('/').to_string();
        let mut headers = HeaderMap::new();
        // The key, or the fresh token of a sign-in (renewed first when it is about to run out).
        let bearer = bearer_of(settings).await?;
        let url = if row.kind == ProviderKind::Anthropic {
            let Some(key) = bearer.as_deref() else { bail!("no API key") };
            let mut value = HeaderValue::from_str(key).map_err(|_| anyhow!("the API key is not a header value"))?;
            value.set_sensitive(true);
            headers.insert("x-api-key", value);
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            format!("{base}/v1/models?limit=100")
        } else {
            if let Some(key) = bearer.as_deref() {
                let mut value = HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|_| anyhow!("the API key is not a header value"))?;
                value.set_sensitive(true);
                headers.insert(AUTHORIZATION, value);
            }
            format!("{base}/models")
        };
        for (name, value) in &settings.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| anyhow!("not a header name: {name}"))?;
            headers.insert(name, HeaderValue::from_str(value).map_err(|_| anyhow!("not a header value: {value}"))?);
        }
        let failed = |error: reqwest::Error| {
            if error.is_timeout() {
                anyhow!("timeout: {url} did not answer in {} s", CHECK_TIMEOUT.as_secs())
            } else {
                anyhow!("cannot reach {url}: {}", describe(&error))
            }
        };
        let response = client().get(&url).headers(headers).timeout(CHECK_TIMEOUT).send().await.map_err(failed)?;
        let status = response.status();
        if !status.is_success() {
            // A service without a list (YandexGPT) still has its suggested models.
            if let (404, Some(preset)) = (status.as_u16(), preset.filter(|preset| preset.suggested.is_some())) {
                return Ok(suggested_of(preset));
            }
            let body = response.text().await.unwrap_or_default();
            bail!("HTTP {}: {}", status.as_u16(), head(&body, 200));
        }
        let body = response.bytes().await.map_err(failed)?;
        let body: Value =
            serde_json::from_slice(&body).map_err(|error| anyhow!("the list of models is not JSON: {error}"))?;
        let listed: Vec<String> = match (body.get("data"), body.get("models")) {
            (Some(Value::Array(models)), _) => {
                models.iter().filter_map(|model| model.get("id").and_then(Value::as_str).map(str::to_string)).collect()
            }
            // ChatGPT's plan lists `models` by `slug`, the ones to show with `visibility: "list"`.
            (_, Some(Value::Array(models))) => models
                .iter()
                .filter(|model| model.get("visibility").and_then(Value::as_str).is_none_or(|seen| seen == "list"))
                .filter_map(|model| model.get("slug").or_else(|| model.get("id")).and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        let mut chat: Vec<String> = if row.kind == ProviderKind::Anthropic {
            listed
        } else {
            listed.into_iter().filter(|id| !NOT_CHAT.is_match(id)).collect()
        };
        if chat.is_empty() {
            return Ok(preset.filter(|preset| preset.suggested.is_some()).map(suggested_of).unwrap_or_default());
        }
        chat.sort_by(|a, b| locale_cmp(a, b));
        chat.dedup();
        Ok(chat)
    }

    /// The check on the card of the provider that was checked; one that took its id since keeps its own card.
    fn save_check(
        &self,
        row: &ProviderRow,
        models: &[String],
        result: ProviderCheck,
    ) -> Result<ProviderCheck, ApiError> {
        let _mutation = self.lock();
        self.db.with(|c| {
            c.execute(
                "UPDATE llm_providers SET models = ?3, last_check = ?4 WHERE id = ?1 AND created_at = ?2",
                params![
                    row.id,
                    row.created_at,
                    json!(models).to_string(),
                    serde_json::to_string(&result).unwrap_or_default()
                ],
            )
        })?;
        self.reload()?;
        Ok(result)
    }

    /// A failure as the owner reads it on the card.
    fn problem_of(&self, error: &anyhow::Error) -> String {
        // The plan's limit, a sign-in to renew: the same words the station says.
        if let Some(failure) = error.downcast_ref::<SpokenFailure>() {
            return failure.phrase.clone();
        }
        let message = self.secrets.redact(&error.to_string());
        if NO_KEY.is_match(&message) {
            return crate::tr!("Нет ключа.");
        }
        if KEY_REFUSED.is_match(&message) {
            return crate::tr!("Ключ не принят.");
        }
        if NOT_FOUND.is_match(&message) {
            return crate::tr!("Модель или адрес не найдены.");
        }
        if REFUSED.is_match(&message) {
            return crate::tr!("Сервис отказал: лимит или пустой счёт.");
        }
        if UNREACHABLE.is_match(&message) {
            return crate::tr!("Адрес не отвечает.");
        }
        if TIMED_OUT.is_match(&message) {
            return crate::tr!("Не ответил за {seconds} секунд.", seconds = CHECK_TIMEOUT.as_secs());
        }
        crate::tr!("Ошибка: {message}", message = head(&message, 200))
    }
}

fn missing() -> ApiError {
    ApiError::not_found(crate::tr!("Такого провайдера нет."))
}

/// A helper's row is the gateway's own: it changes with the helper.
fn helpers_own() -> ApiError {
    ApiError::conflict(crate::tr!("Модель Турбо меняется вместе с Турбо: «Настройки» → «Дом»."))
}

/// A provider as the one a sign-in's key is for.
fn target_of(row: &ProviderRow) -> Target {
    Target::Provider { id: row.id.clone(), created_at: row.created_at.clone() }
}

/// The token sign-in of a preset (ChatGPT's plan), if its access is one.
fn token_flow(preset: &str) -> Option<&'static tokens::TokenFlow> {
    match preset_of(preset)?.sign_in.as_ref()?.flow {
        Flow::Tokens(flow) => Some(flow),
        Flow::KeyExchange { .. } => None,
    }
}

/// A provider's client; a panel provider is anthropic or openai, so there always is one.
fn build(settings: &LlmSettings) -> Arc<dyn LlmClient> {
    create_llm(settings).unwrap_or_else(|| Arc::new(crate::dialog::llm::echo::EchoLlm))
}

/// The preset's id, or with the first free number: `ollama`, `ollama-2`, …
fn new_id(taken: &[String], base: &str) -> String {
    let taken = |id: &str| taken.iter().any(|known| known == id);
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).unwrap_or_else(|| base.to_string())
}

/// A row's questions leave the house: Claude always, a helper's model when its helper is not at home (the link's local
/// bridge would look like home), others unless their address is at home.
fn cloud_of_row(row: &ProviderRow) -> bool {
    if row.kind == ProviderKind::Anthropic {
        return true;
    }
    if row.helper().is_some() {
        return row.stored_reach() != Reach::Home;
    }
    row.base_url.as_deref().is_none_or(|url| !is_local_url(Some(url)))
}

/// A row's questions leave the house now: a helper's row by the road its helper is asked by now (a computer at home
/// reached through its tunnel for a while goes out), any other as stored (`cloud_of_row`).
fn cloud_now(row: &ProviderRow, seen: &Seen) -> bool {
    match row.helper().and_then(|id| seen.helper(id)) {
        Some(helper) => helper.reach != Reach::Home,
        None => cloud_of_row(row),
    }
}

/// A row's title for people: a helper's row's names its helper as it is called now — or as it was, once gone.
fn title_of(row: &ProviderRow, seen: &Seen) -> String {
    match row.helper() {
        Some(id) => {
            let name = seen.helper(id).map(|helper| helper.name.as_str()).unwrap_or(row.helper_name());
            crate::tr!("Модель: {name}", name = name)
        }
        None => row.title.clone(),
    }
}

/// The address: the preset's own, or the given one when the preset lets it change.
fn address(preset: &ProviderPreset, value: Option<&Value>) -> Result<Option<String>, ApiError> {
    if !preset.editable_url {
        return Ok(preset.base_url.map(str::to_string));
    }
    let given = text(value, 300);
    let url = if given.is_empty() { preset.base_url.unwrap_or("").to_string() } else { given };
    if url.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Укажите адрес сервиса.")));
    }
    match url::Url::parse(&url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => Ok(Some(url.trim_end_matches('/').to_string())),
        _ => Err(ApiError::bad_request(crate::tr!("Адрес должен начинаться с http:// или https://."))),
    }
}

fn fields(preset: &ProviderPreset, value: Option<&Value>) -> BTreeMap<String, String> {
    let given = value.and_then(Value::as_object);
    preset
        .fields
        .iter()
        .filter_map(|field| {
            let entry = text(given.and_then(|given| given.get(field.id)), 200);
            (!entry.is_empty()).then(|| (field.id.to_string(), entry))
        })
        .collect()
}

// Who answers (docs/plans/home-and-helpers.md, block 1, stage 4).

/// Where a pick's model is.
#[derive(Clone)]
enum Where<'a> {
    Row(&'a ProviderRow),
    /// The .env model: its slot for the profile.
    Env(ModelSlot),
}

/// The helper whose model a pick is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Whose {
    /// Its entry's id; `self` for a model server of this computer (stage 6).
    id: String,
    name: String,
    builtin: bool,
    /// On the line, its model not ready (its model server closed there): the model is silent, not the helper away.
    silent: bool,
}

impl Whose {
    fn of(helper: &ModelHelper) -> Whose {
        let silent = !helper.builtin && !helper.up && helper.state == HelperState::Up;
        Whose { id: helper.id.clone(), name: helper.name.clone(), builtin: helper.builtin, silent }
    }
}

/// A pick as it may be asked now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Judged {
    /// The regulator lets its reach out, and it is no helper's model or its helper is on the line.
    Open,
    /// A helper's model whose helper is not on the line, or no longer gives it.
    Away,
    /// The regulator «Что может уходить из дома» keeps its reach in.
    Shut,
}

/// One pick — the profile's chosen model, the backup, the next helper's model — as the resolver judges it.
#[derive(Clone)]
struct Pick<'a> {
    at: Where<'a>,
    /// The row's id, or `env`.
    provider: String,
    model: String,
    reach: Reach,
    whose: Option<Whose>,
    judged: Judged,
}

/// What answers when the chosen model, which may be asked, gives no answer.
enum Behind<'a> {
    Nothing,
    Model(Pick<'a>),
    /// The words: nothing else may be asked.
    Words(String),
}

/// Why the chosen model does not answer.
enum Why {
    Away(Whose),
    Shut,
}

impl Why {
    /// For «Где сейчас».
    fn text(&self) -> String {
        match self {
            Why::Away(whose) if whose.builtin => crate::core::tx(BUILTIN_AWAY),
            Why::Away(whose) if whose.silent => crate::tr!("{name}: модель сейчас не отвечает", name = whose.name),
            Why::Away(whose) => crate::tr!("{name} не на связи", name = whose.name),
            Why::Shut => crate::core::tx(CLOUDS_SHUT),
        }
    }
}

/// Who answers a profile now.
enum Answer<'a> {
    /// The chosen model, and what stands behind it.
    Chosen(Pick<'a>, Behind<'a>),
    /// Another model in its place, at once: the backup, or the next helper with a model.
    Instead(Pick<'a>, Why),
    /// None may be asked: the words say why.
    Words(String),
}

fn judged(seen: &Seen, reach: Reach, up: bool) -> Judged {
    if !seen.lets(reach) {
        Judged::Shut
    } else if !up {
        Judged::Away
    } else {
        Judged::Open
    }
}

/// A row's model as a pick: a helper's row by its helper (gone from the list — away, under its stored name and reach),
/// a model server of this computer by its model helper (stage 6), any other by its address. Without the helpers known
/// every row is open, as before them.
fn judge_row<'a>(row: &'a ProviderRow, model: &str, seen: &Seen) -> Pick<'a> {
    let by_address = || (None, true, if cloud_of_row(row) { Reach::Foreign } else { Reach::Home });
    let (whose, up, reach) = match (seen.known, row.helper()) {
        (false, _) => by_address(),
        (true, Some(id)) => match seen.helper(id) {
            Some(helper) => (Some(Whose::of(helper)), helper.up, helper.reach),
            None => {
                let gone =
                    Whose { id: id.to_string(), name: row.helper_name().to_string(), builtin: false, silent: false };
                (Some(gone), false, row.stored_reach())
            }
        },
        (true, None) => match seen.named(&row.id) {
            Some(helper) => (Some(Whose::of(helper)), helper.up, helper.reach),
            None => by_address(),
        },
    };
    Pick {
        at: Where::Row(row),
        provider: row.id.clone(),
        model: model.to_string(),
        reach,
        whose,
        judged: judged(seen, reach, up),
    }
}

/// The .env model as a pick: by its address, never a helper's.
fn judge_env<'a>(slot: ModelSlot, seen: &Seen) -> Pick<'a> {
    let reach = if slot.cloud { Reach::Foreign } else { Reach::Home };
    Pick {
        provider: ENV_PROVIDER.into(),
        model: slot.model.clone(),
        reach,
        whose: None,
        judged: judged(seen, reach, true),
        at: Where::Env(slot),
    }
}

/// «The next helper»: a helper with a model, in the household's order (this computer's model servers first, stage 6),
/// on the line and let out, other than the chosen model's own row, whose model is known — the chosen model when its
/// row's list holds it, else the first of its list. One without a known model is passed by, and so is one whose road
/// «Выход в интернет» refuses now (`refused`, by the row's id).
fn next_helper<'a>(
    rows: &'a [ProviderRow],
    seen: &Seen,
    chosen: &Pick,
    refused: &dyn Fn(&str) -> bool,
) -> Option<Pick<'a>> {
    seen.helpers.iter().filter(|helper| helper.up && seen.lets(helper.reach)).find_map(|helper| {
        let row = match &helper.provider {
            Some(id) => rows.iter().find(|row| row.id == *id),
            None => rows.iter().find(|row| row.helper() == Some(helper.id.as_str())),
        }?;
        if row.id == chosen.provider || refused(&row.id) {
            return None;
        }
        let model = row.models.iter().find(|model| **model == chosen.model).or(row.models.first())?;
        Some(judge_row(row, model, seen))
    })
}

/// The words as a model: none may be asked, and they say why at once.
fn words_slot(phrase: String) -> ModelSlot {
    ModelSlot {
        client: Arc::new(Unreachable::new(phrase)),
        model: String::new(),
        provider: WORDS_PROVIDER.into(),
        kind: DialogProvider::None,
        title: crate::tr!("нет модели на связи"),
        cloud: false,
        backup: None,
    }
}

/// W1 — the chosen helper's model is away and there is no backup.
fn away_without_backup(whose: &Whose) -> String {
    if whose.builtin {
        return crate::tr!("Модель на этом компьютере не отвечает, а запасной модели нет.");
    }
    if whose.silent {
        return crate::tr!("{name}: модель сейчас не отвечает, а запасной модели нет.", name = whose.name);
    }
    crate::tr!("{name} не на связи, а запасной модели нет.", name = whose.name)
}

/// W2 — the chosen helper's model is away and the regulator keeps the backup in.
fn away_with_backup_shut(whose: &Whose) -> String {
    if whose.builtin {
        return crate::tr!(
            "Модель на этом компьютере не отвечает, а облака сейчас закрыты — запасная модель не ответит."
        );
    }
    if whose.silent {
        return crate::tr!(
            "{name}: модель сейчас не отвечает, а облака сейчас закрыты — запасная модель не ответит.",
            name = whose.name
        );
    }
    crate::tr!("{name} не на связи, а облака сейчас закрыты — запасная модель не ответит.", name = whose.name)
}

/// W2 of «Выход в интернет» — the chosen helper's model is away, and the backup goes through a VPN that does not
/// answer now (src/egress), or waits for one: it would only say so.
fn away_with_backup_refused(whose: &Whose) -> String {
    if whose.builtin {
        return crate::tr!("Модель на этом компьютере не отвечает, а запасная модель недоступна: VPN не отвечает.");
    }
    if whose.silent {
        return crate::tr!(
            "{name}: модель сейчас не отвечает, а запасная модель недоступна: VPN не отвечает.",
            name = whose.name
        );
    }
    crate::tr!("{name} не на связи, а запасная модель недоступна: VPN не отвечает.", name = whose.name)
}

/// W3 — the chosen helper's model and the backup's helper are both away. Two model servers of this computer (stage 6)
/// are said together; a helper on the line whose model is silent is no helper away.
fn both_away(whose: &Whose, backup: Option<&Whose>) -> String {
    if whose.builtin && backup.is_some_and(|backup| backup.builtin) {
        return crate::tr!("Ни основная, ни запасная модель на этом компьютере сейчас не отвечают.");
    }
    if whose.silent || backup.is_some_and(|backup| backup.silent) {
        return crate::tr!("Ни основная, ни запасная модель сейчас не отвечают.");
    }
    let named = |whose: &Whose| {
        if whose.builtin {
            crate::tr!("модель на этом компьютере")
        } else {
            whose.name.clone()
        }
    };
    let backup = backup.map(named).unwrap_or_default();
    crate::tr!("Ни {name}, ни {backup} сейчас не на связи.", name = named(whose), backup = backup)
}

/// A pick on the same helper as another: a backup there is away whenever that one is. Two model servers of this
/// computer (stage 6) are two: each is looked at by itself, and one stands in for the other.
fn beside(pick: &Pick, other: &Pick) -> bool {
    pick.whose.as_ref().is_some_and(|whose| {
        other.whose.as_ref().is_some_and(|theirs| {
            theirs.id == whose.id && !(whose.builtin && theirs.builtin && pick.provider != other.provider)
        })
    })
}

/// Whether the main model — fast or smart — is a helper's: a helper's row, or a model server of this computer.
fn main_is_helpers(rows: &[ProviderRow], choice: &Brain, seen: &Seen) -> bool {
    [&choice.fast, &choice.smart].into_iter().flatten().any(|pick| {
        rows.iter().any(|row| row.id == pick.provider && (row.helper().is_some() || seen.named(&row.id).is_some()))
    })
}

/// Whether the gateway would set the backup by itself now (default 6): the main model is a helper's, there is no
/// backup, a person did not say «не нужна», and the regulator lets the clouds out.
fn wants_backup(rows: &[ProviderRow], choice: &Brain, seen: &Seen) -> bool {
    seen.known
        && main_is_helpers(rows, choice, seen)
        && choice.backup.is_none()
        && choice.backup_by != Some(BackupBy::Declined)
        && seen.lets(Reach::Foreign)
}

/// The backup the gateway may take: the first cloud by keys in the panel's order, with its key or its sign-in, whose
/// service names a fast model exactly — with its list known, the first hint the list holds by its exact id (a leading
/// `models/` cut, the listed id taken); without one, the preset's suggested fast model. Also the clouds with hints whose
/// list is not known yet: theirs is asked for. A cloud «Выход в интернет» refuses now (`refused`, by the row's id) is
/// passed by, its list not asked: it would not answer in the computer's place.
fn candidate<'a>(
    rows: &'a [ProviderRow],
    secrets: &SecretsService,
    refused: &dyn Fn(&str) -> bool,
) -> (Option<(String, String)>, Vec<&'a ProviderRow>) {
    let mut unlisted = Vec::new();
    for row in rows.iter().filter(|row| row.helper().is_none() && cloud_of_row(row) && !refused(&row.id)) {
        let Some(preset) = preset_of(&row.preset).filter(|preset| !preset.fast.is_empty()) else { continue };
        if !secrets.has(&key_of(&row.id)) && !secrets.has(&record_id(&row.id)) {
            continue;
        }
        if row.models.is_empty() {
            if let Some(fast) =
                preset.suggested.as_ref().map(|suggested| suggested.fast).filter(|fast| !fast.is_empty())
            {
                return (Some((row.id.clone(), fast.to_string())), unlisted);
            }
            unlisted.push(row);
            continue;
        }
        let listed = preset
            .fast
            .iter()
            .find_map(|hint| row.models.iter().find(|model| model.strip_prefix("models/").unwrap_or(model) == *hint));
        if let Some(model) = listed {
            return (Some((row.id.clone(), model.clone())), unlisted);
        }
    }
    (None, unlisted)
}

/// How far a helper's row reaches, as its options say it.
fn reach_word(reach: Reach) -> &'static str {
    if reach == Reach::Home {
        "home"
    } else {
        "mine"
    }
}

/// A helper's row's options: whose and how far.
fn helper_options(helper: &ModelHelper) -> BTreeMap<String, String> {
    BTreeMap::from([("helper".to_string(), helper.id.clone()), ("reach".to_string(), reach_word(helper.reach).into())])
}

/// What a helper's row stores: a plain entry's model route and key, today's form (a station rolled back to 0.11
/// still reaches the computer); nothing for a linked entry — its bridge and token are this start's —, which the table
/// keeps as the stand-in `HELPER_NO_ADDRESS`.
fn stored_of(helper: &ModelHelper) -> (Option<String>, Option<String>) {
    match helper.linked {
        true => (None, None),
        false => (Some(helper.base_url.clone()), helper.key.clone()),
    }
}

/// The station's old row for its computer's model — «Модель на компьютере» at a helper's model route — or a custom row
/// at a helper's: becomes its helper's row in place.
fn old_row(row: &ProviderRow, helpers: &[ModelHelper]) -> bool {
    let at_route = |url: &str| url.trim_end_matches('/').ends_with("/home/v1/llm");
    let own_road = |helper: &&ModelHelper| helper.via.is_none() && !helper.linked;
    row.preset != HELPER_PRESET
        && ((row.title == crate::home_server::BRAIN_TITLE && row.base_url.as_deref().is_some_and(at_route))
            || (row.preset == "custom-openai"
                && helpers.iter().filter(own_road).any(|helper| row.base_url.as_deref() == Some(&helper.base_url))))
}

impl BrainService {
    /// Who answers a profile now: the table of the plan's stage 4, over the rows, the choice and the helpers as seen.
    /// None — no model at all: nothing chosen and none in .env.
    fn resolve<'a>(
        &self,
        rows: &'a [ProviderRow],
        choice: &'a Brain,
        seen: &Seen,
        profile: ModelProfile,
    ) -> Option<Answer<'a>> {
        let chosen = match Self::picked(rows, choice, profile) {
            Some((row, pick)) => judge_row(row, &pick.model, seen),
            None => judge_env(self.env.slot(profile)?, seen),
        };
        let backup = choice
            .backup
            .as_ref()
            .and_then(|pick| match rows.iter().find(|row| row.id == pick.provider) {
                Some(row) => Some(judge_row(row, &pick.model, seen)),
                None if pick.provider == ENV_PROVIDER => self.env.slot(profile).map(|slot| judge_env(slot, seen)),
                None => None,
            })
            // The chosen model itself is no backup.
            .filter(|backup| (&backup.provider, &backup.model) != (&chosen.provider, &chosen.model));
        let refused = |provider: &str| self.refused(provider);
        if chosen.judged == Judged::Open {
            // A model server of this computer on the line says its own failure, as before the helpers; once its look
            // finds it closed, another answers in its place at once (stage 6). A chosen model whose VPN does not answer
            // is judged as before the helpers: the station gives its question to Alice (`brain_refused`), else it says
            // «VPN не отвечает» and the backup behind it takes the round.
            let helpers_model = chosen.whose.as_ref().is_some_and(|whose| !whose.builtin);
            let behind = match backup {
                Some(backup) if backup.judged == Judged::Open => Behind::Model(backup),
                None if helpers_model => match next_helper(rows, seen, &chosen, &refused) {
                    Some(next) => Behind::Model(next),
                    None => Behind::Words(crate::tr!(
                        "{name}: модель не ответила, а запасной нет.",
                        name = chosen.whose.as_ref().map(|whose| whose.name.as_str()).unwrap_or_default()
                    )),
                },
                // A cloud's own failure says itself (the plan's limit), or the error phrase does: no words behind it.
                _ if chosen.reach == Reach::Foreign => Behind::Nothing,
                None => Behind::Nothing,
                Some(backup) if backup.judged == Judged::Shut => Behind::Words(crate::tr!(
                    "Модель не ответила, а облака сейчас закрыты — запасная модель не ответит."
                )),
                Some(backup) => Behind::Words(match &backup.whose {
                    // A backup on a closed server of this computer, or at a helper on the line whose model is silent.
                    Some(whose) if whose.builtin || whose.silent => {
                        crate::tr!("Модель не ответила, а запасная модель сейчас не отвечает.")
                    }
                    whose => crate::tr!(
                        "Модель не ответила, а {backup} с запасной моделью не на связи.",
                        backup = whose.as_ref().map(|whose| whose.name.as_str()).unwrap_or_default()
                    ),
                }),
            };
            return Some(Answer::Chosen(chosen, behind));
        }
        let why = match (&chosen.judged, &chosen.whose) {
            (Judged::Away, Some(whose)) => Why::Away(whose.clone()),
            _ => Why::Shut,
        };
        // A backup on the chosen model's own helper stands in for a model that fails there, never for its helper away:
        // it is no backup now.
        let backup = backup.filter(|backup| !beside(backup, &chosen));
        // No request goes to the helper that is away: the backup at once, else the next helper with a model — never one
        // whose VPN does not answer now (src/egress): it would only say so, and the station in «Вместе» gives a
        // question nobody may answer to Alice (the owner, 15.11), as `brain_refused` does for a chosen model.
        if let Some(backup) =
            backup.clone().filter(|backup| backup.judged == Judged::Open && !refused(&backup.provider))
        {
            return Some(Answer::Instead(backup, why));
        }
        if let Some(next) = next_helper(rows, seen, &chosen, &refused) {
            return Some(Answer::Instead(next, why));
        }
        let words = match (&why, &backup) {
            (Why::Shut, _) => crate::tr!("Модель в облаке, а облака сейчас закрыты."),
            (Why::Away(whose), None) => away_without_backup(whose),
            (Why::Away(whose), Some(backup)) if backup.judged == Judged::Shut => away_with_backup_shut(whose),
            // Let out and on the line, and passed by all the same: its VPN does not answer.
            (Why::Away(whose), Some(backup)) if backup.judged == Judged::Open => away_with_backup_refused(whose),
            (Why::Away(whose), Some(backup)) => both_away(whose, backup.whose.as_ref()),
        };
        Some(Answer::Words(words))
    }

    /// A pick's slot, without a backup. The caller holds `mutation`.
    fn slot_of_pick(&self, pick: &Pick, seen: &Seen) -> ModelSlot {
        match &pick.at {
            Where::Row(row) => self.slot_of_row(row, &pick.model, seen),
            Where::Env(slot) => slot.clone(),
        }
    }

    /// The slot that answers, with what stands behind it; the words — answering, or behind — only with `words`. The
    /// caller holds `mutation`.
    fn slot_of_answer(&self, answer: Answer, seen: &Seen, words: bool) -> Option<ModelSlot> {
        match answer {
            Answer::Chosen(chosen, behind) => {
                let mut slot = self.slot_of_pick(&chosen, seen);
                slot.backup = match behind {
                    Behind::Nothing => None,
                    Behind::Model(backup) => Some(self.slot_of_pick(&backup, seen)),
                    Behind::Words(phrase) => words.then(|| words_slot(phrase)),
                }
                .map(|backup| Arc::new(Backup::new(backup)));
                Some(slot)
            }
            Answer::Instead(pick, _) => Some(self.slot_of_pick(&pick, seen)),
            Answer::Words(phrase) => words.then(|| words_slot(phrase)),
        }
    }

    /// The slot of a profile that may be asked: None when only the words would answer; words behind it left out. The
    /// night's reflection and the router read it.
    pub fn reachable_slot(&self, profile: ModelProfile) -> Option<ModelSlot> {
        let seen = self.seen();
        let _mutation = self.lock();
        let (rows, choice) = self.snapshot();
        let answer = self.resolve(&rows, &choice, &seen, profile)?;
        self.slot_of_answer(answer, &seen, false)
    }

    /// One model of a row, picked apart from the profiles (the memory's model, the workshop's agent, an app's): its
    /// slot, without a backup — or why not now: Err(None) — no such row; Err(Some(why)) — the row is there and waits,
    /// its helper away or gone («Компьютер дома не на связи»), or kept in by the regulator.
    pub fn pick_of(&self, provider: &str, model: &str) -> Result<ModelSlot, Option<String>> {
        let seen = self.seen();
        let _mutation = self.lock();
        let (rows, _) = self.snapshot();
        let row = rows.iter().find(|row| row.id == provider).ok_or(None)?;
        let pick = judge_row(row, model, &seen);
        match (pick.judged, pick.whose) {
            (Judged::Open, _) => Ok(self.slot_of_row(row, model, &seen)),
            (Judged::Away, Some(whose)) => Err(Some(Why::Away(whose).text())),
            _ => Err(Some(Why::Shut.text())),
        }
    }

    /// Who answers a profile now — the same decision as the slot's, no client built: for «Где сейчас» and «Спросить
    /// мозг».
    pub fn now(&self, profile: ModelProfile) -> BrainNow {
        let seen = self.seen();
        let (rows, choice) = self.snapshot();
        let off = BrainNow {
            state: Answering::Off,
            provider: None,
            title: None,
            model: None,
            reach: None,
            helper: None,
            reason: None,
        };
        let Some(answer) = self.resolve(&rows, &choice, &seen, profile) else { return off };
        let of = |pick: &Pick, state: Answering, reason: Option<String>| BrainNow {
            state,
            provider: Some(pick.provider.clone()),
            title: Some(match &pick.at {
                Where::Row(row) => title_of(row, &seen),
                Where::Env(slot) => slot.title.clone(),
            }),
            model: Some(pick.model.clone()),
            reach: Some(pick.reach),
            helper: pick.whose.as_ref().map(|whose| whose.id.clone()),
            reason,
        };
        match answer {
            Answer::Chosen(chosen, _) => of(&chosen, Answering::Ready, None),
            Answer::Instead(pick, why) => of(&pick, Answering::Fallback, Some(why.text())),
            Answer::Words(phrase) => BrainNow { reason: Some(phrase), ..off },
        }
    }

    // The helpers' rows and the default backup (docs/plans/home-and-helpers.md, stage 4).

    /// Whether a choice answers with this helper's model: its row is the fast, the smart or the backup pick.
    pub fn answers_with_helper(&self, helper: &str) -> bool {
        let (rows, choice) = self.snapshot();
        let answers = [&choice.fast, &choice.smart, &choice.backup]
            .into_iter()
            .flatten()
            .any(|pick| rows.iter().any(|row| row.id == pick.provider && row.helper() == Some(helper)));
        answers
    }

    /// The helpers' models among the rows: one row per helper with a model — «Модель: <helper>», the hidden preset
    /// `helper`, options `{helper, reach}`, its id `helper-<entry>` —, made once a look says the helper has one (a
    /// start, which has read nobody yet, makes and drops nothing); changed with its name, reach, address and key;
    /// removed when the helper leaves the list, is switched off or loses the model — unless a choice answers with it:
    /// then it stays, taken as away and never asked, and keeps neither the gone helper's address nor its key. The
    /// station's old row «Модель на компьютере» becomes its helper's in place, under its id, so every pick of it
    /// (`brain`, `workshop.agent`, the memory's, the apps') still points at it. A helper asked through its later entry
    /// for now (`via`) keeps its row as stored but for the name. A model server of this computer (stage 6) is the
    /// person's own row: never touched. One `brain.changed` «helper.model» for it all.
    pub fn keep_helper_models(self: &Arc<Self>) -> Result<(), ApiError> {
        let Some(source) = self.helpers.get().cloned() else { return Ok(()) };
        let helpers: Vec<ModelHelper> =
            source.model_helpers().into_iter().filter(|helper| helper.provider.is_none() && !helper.builtin).collect();
        let mut changed: Vec<String> = Vec::new();
        {
            let _mutation = self.lock();
            let rows = self.load_rows()?;
            let olds: Vec<&ProviderRow> = rows.iter().filter(|row| old_row(row, &helpers)).collect();
            let mut taken: Vec<&str> = rows.iter().filter_map(ProviderRow::helper).collect();
            for old in &olds {
                let at = |helper: &&ModelHelper| {
                    helper.via.is_none() && !helper.linked && old.base_url.as_deref() == Some(&helper.base_url)
                };
                let only = || (helpers.len() == 1 && olds.len() == 1 && helpers[0].via.is_none()).then(|| &helpers[0]);
                let Some(helper) = helpers.iter().find(at).or_else(only) else { continue };
                if taken.contains(&helper.id.as_str()) {
                    continue;
                }
                self.write_helper_row(&old.id, helper)?;
                taken.push(&helper.id);
                changed.push(old.id.clone());
            }
            let rows = if changed.is_empty() { rows } else { self.load_rows()? };
            for helper in &helpers {
                match rows.iter().find(|row| row.helper() == Some(helper.id.as_str())) {
                    Some(row) if !self.row_is(row, helper) => {
                        self.write_helper_row(&row.id, helper)?;
                        changed.push(row.id.clone());
                    }
                    Some(_) => {}
                    // Not before its first look says it has one, and from its own road only.
                    None if helper.read && helper.via.is_none() => changed.push(self.insert_helper_row(helper)?),
                    None => {}
                }
            }
            let choice = self.settings.peek(&BRAIN)?.unwrap_or_default();
            let answering = |id: &str| {
                [&choice.fast, &choice.smart, &choice.backup].into_iter().flatten().any(|pick| pick.provider == id)
            };
            let gone =
                |row: &&ProviderRow| row.helper().is_some_and(|id| !helpers.iter().any(|helper| helper.id == id));
            for row in rows.iter().filter(gone) {
                if !answering(&row.id) {
                    self.db.with(|c| c.execute("DELETE FROM llm_providers WHERE id = ?1", params![row.id]))?;
                    self.secrets.delete_all(&format!("llm:{}:", row.id))?;
                    changed.push(row.id.clone());
                } else if row.base_url.is_some() || self.secrets.has(&key_of(&row.id)) {
                    // Kept while it answers, never asked: what reached the gone helper does not travel on with it. The
                    // stand-in takes its address: a version from before the helpers would read none as OpenAI's.
                    self.db.with(|c| {
                        c.execute(
                            "UPDATE llm_providers SET base_url = ?3, updated_at = ?2 WHERE id = ?1",
                            params![row.id, now_iso(), HELPER_NO_ADDRESS],
                        )
                    })?;
                    self.secrets.delete(&key_of(&row.id))?;
                    changed.push(row.id.clone());
                }
            }
            if !changed.is_empty() {
                self.reload()?;
            }
        }
        if let Some(first) = changed.first() {
            tracing::info!(target: "brain", "the helpers' models: {}", changed.join(", "));
            self.events.emit(GatewayEvent::BrainChanged {
                change: "helper.model".into(),
                provider_id: Some(first.clone()),
                actor: None,
            });
        }
        // «The next helper» answers only with a model it knows: the lists of the helpers' rows that have none, asked
        // while their helper is on the line — never past the regulator.
        let (rows, _) = self.snapshot();
        for row in rows.iter().filter(|row| row.models.is_empty()) {
            let on_line = row
                .helper()
                .and_then(|id| helpers.iter().find(|helper| helper.id == id))
                .is_some_and(|helper| helper.up && source.lets(helper.reach));
            if on_line {
                self.list_in_background(row, false);
            }
        }
        Ok(())
    }

    /// Whether a helper's row is as its helper is now: by its name alone while it is asked through its later entry.
    fn row_is(&self, row: &ProviderRow, helper: &ModelHelper) -> bool {
        let named = row.title == format!("{HELPER_TITLE}{}", helper.name);
        if helper.via.is_some() {
            return named;
        }
        let (url, key) = stored_of(helper);
        named
            && row.options == helper_options(helper)
            && row.base_url == url
            && self.secrets.get(&key_of(&row.id)) == key
    }

    /// A row made its helper's as the helper is now; one asked through its later entry takes only the name. The
    /// caller holds `mutation`.
    fn write_helper_row(&self, id: &str, helper: &ModelHelper) -> Result<(), ApiError> {
        let title = format!("{HELPER_TITLE}{}", helper.name);
        if helper.via.is_some() {
            self.db.with(|c| {
                c.execute(
                    "UPDATE llm_providers SET title = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, title, now_iso()],
                )
            })?;
            return Ok(());
        }
        let (url, key) = stored_of(helper);
        let options = json!(helper_options(helper)).to_string();
        self.db.with(|c| {
            c.execute(
                "UPDATE llm_providers SET preset = ?2, kind = 'openai', title = ?3, base_url = ?4, options = ?5,
                 updated_at = ?6 WHERE id = ?1",
                params![id, HELPER_PRESET, title, url.as_deref().unwrap_or(HELPER_NO_ADDRESS), options, now_iso()],
            )
        })?;
        self.secrets.set(&key_of(id), key.as_deref())
    }

    /// A new row for a helper's model; its id, the helper's own (`helper-<entry>`): a pick of a row gone with its
    /// helper never passes to another helper's row, and the same helper switched on again gets its row back under it.
    /// The caller holds `mutation`.
    fn insert_helper_row(&self, helper: &ModelHelper) -> Result<String, ApiError> {
        let id = new_id(&self.ids()?, &format!("{HELPER_PRESET}-{}", helper.id));
        self.secrets.delete_all(&format!("llm:{id}:"))?;
        self.secrets.delete(&record_id(&id))?;
        let (url, key) = stored_of(helper);
        let options = json!(helper_options(helper)).to_string();
        let title = format!("{HELPER_TITLE}{}", helper.name);
        self.db.with(|c| {
            c.execute(
                "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, models, last_check,
                 created_at, updated_at) VALUES (?1, ?2, 'openai', ?3, ?4, ?5, '[]', NULL, ?6, ?6)",
                params![id, HELPER_PRESET, title, url.as_deref().unwrap_or(HELPER_NO_ADDRESS), options, now_iso()],
            )
        })?;
        self.secrets.set(&key_of(&id), key.as_deref())?;
        Ok(id)
    }

    /// A row's model list asked in the background, the way «Обновить» asks it: once per start when it comes; one that
    /// failed is asked again at the next occasion (a helper up or away, a change of the helpers or of the choice), one
    /// being asked is not asked twice. `then_backup` — the default backup is looked at again when it comes.
    fn list_in_background(self: &Arc<Self>, row: &ProviderRow, then_backup: bool) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let asked = (row.id.clone(), row.created_at.clone());
        if !self.listed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(asked.clone()) {
            return;
        }
        let (brain, id) = (Arc::downgrade(self), row.id.clone());
        runtime.spawn(async move {
            let Some(brain) = brain.upgrade() else { return };
            match brain.models(&id).await {
                Ok(_) if then_backup => brain.keep_backup(),
                Ok(_) => {}
                Err(error) => {
                    brain.listed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&asked);
                    let error = brain.secrets.redact(&format!("{error:#}"));
                    tracing::debug!(target: "brain", "{id}: its models are not listed, asked again later: {error}");
                }
            }
        });
    }

    /// «Запасная» by the gateway itself (docs/plans/home-and-helpers.md, default 6): while the main model is a helper's
    /// — it can be away while the gateway runs —, there is no backup, a person did not say «не нужна» and the regulator
    /// lets the clouds out, the first cloud by keys with a fast model named exactly becomes the backup (`candidate`);
    /// clouds whose list is not known yet are asked for it, once per start, and this runs again when it comes. Only on
    /// a gateway that keeps its own settings. «Мозг» says it in its line: it may cost money.
    pub fn keep_backup(self: &Arc<Self>) {
        if !self.owns() {
            return;
        }
        let seen = self.seen();
        let (rows, choice) = self.snapshot();
        if !wants_backup(&rows, &choice, &seen) {
            return;
        }
        let (taken, unlisted) = candidate(&rows, &self.secrets, &|provider| self.refused(provider));
        let Some((provider, model)) = taken else {
            for row in unlisted {
                self.list_in_background(row, true);
            }
            return;
        };
        let set = || -> Result<bool, ApiError> {
            let _mutation = self.lock();
            let rows = self.load_rows()?;
            let mut next = self.settings.peek(&BRAIN)?.unwrap_or_default();
            // As stored now: a person may have chosen meanwhile.
            if !wants_backup(&rows, &next, &seen) || !rows.iter().any(|row| row.id == provider) {
                return Ok(false);
            }
            next.backup = Some(BrainPick { provider: provider.clone(), model: model.clone() });
            next.backup_by = Some(BackupBy::Gateway);
            self.settings.set(&BRAIN, &next, None)?;
            self.reload()?;
            Ok(true)
        };
        match set() {
            Ok(true) => {
                tracing::info!(target: "brain", "the backup chosen by the gateway: {model} via {provider}");
                self.events.emit(GatewayEvent::BrainChanged {
                    change: "backup.auto".into(),
                    provider_id: Some(provider),
                    actor: None,
                });
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(target: "brain", "the backup is not set: {}", error.message),
        }
    }

    /// «Мозг»'s line about the backup while the main model is a helper's. A backup on the main model's own helper is
    /// away with it: «Мозг» asks for another, as with none (the gateway never puts its own in place of a person's).
    fn note(&self, rows: &[ProviderRow], choice: &Brain, seen: &Seen) -> Option<BrainNote> {
        let main = seen.known && main_is_helpers(rows, choice, seen);
        let declined = choice.backup_by == Some(BackupBy::Declined);
        let judge = |pick: &BrainPick| match rows.iter().find(|row| row.id == pick.provider) {
            Some(row) => Some(judge_row(row, &pick.model, seen)),
            None if pick.provider == ENV_PROVIDER => {
                self.env.slot(ModelProfile::Fast).map(|slot| judge_env(slot, seen))
            }
            None => None,
        };
        let backup = choice.backup.as_ref().and_then(judge);
        let mains: Vec<Pick> = [&choice.fast, &choice.smart].into_iter().flatten().filter_map(judge).collect();
        let beside_main = backup.as_ref().is_some_and(|backup| mains.iter().any(|main| beside(backup, main)));
        let usable = backup.as_ref().is_some_and(|backup| backup.judged != Judged::Shut) && !beside_main;
        if main && !usable && !declined && !seen.lets(Reach::Foreign) {
            let text = crate::tr!(
                "Облака закрыты регулятором «Что может уходить из дома»: когда основная модель не на связи, мозг скажет об этом словами."
            );
            return Some(BrainNote { kind: "backupClosed", text });
        }
        if let (Some(pick), Some(BackupBy::Gateway)) = (&choice.backup, choice.backup_by) {
            let provider = rows
                .iter()
                .find(|row| row.id == pick.provider)
                .map(|row| title_of(row, seen))
                .unwrap_or_else(|| pick.provider.clone());
            let text = crate::tr!(
                "Запасную модель центр выбрал сам: {model} ({provider}). Она отвечает, когда основная не на связи. Это облако по вашему ключу: ответы могут стоить денег, а папки без галочки «Модель может читать имена и файлы» модель теперь не читает.",
                model = pick.model,
                provider = provider
            );
            return Some(BrainNote { kind: "backupAuto", text });
        }
        if main
            && (backup.is_none() || beside_main)
            && !declined
            && seen.lets(Reach::Foreign)
            && (beside_main || candidate(rows, &self.secrets, &|provider| self.refused(provider)).0.is_none())
        {
            let text = if beside_main {
                crate::tr!(
                    "«Запасная» стоит у того же Турбо, что и основная модель: пропадёт Турбо — пропадут обе. Выберите другую «Запасную» — иначе в это время мозг только скажет, что модели нет."
                )
            } else {
                crate::tr!(
                    "Основная модель бывает не на связи, а запасной нет, и центру не из чего выбрать её самому: нужен ключ облака, чью быструю модель он знает по имени (Claude, OpenAI, Gemini…). Выберите «Запасную» сами — иначе в это время мозг только скажет, что модели нет."
                )
            };
            return Some(BrainNote { kind: "backupNeeded", text });
        }
        None
    }
}

impl BrainSource for BrainService {
    /// The profile's model, with the backup when one is chosen and it is another model — judged with the helpers: a
    /// chosen model whose helper is away gives way at once to the backup, the next helper with a model, or the words.
    fn slot(&self, profile: ModelProfile) -> Option<ModelSlot> {
        // The helpers first; then the row and the key its client is built with, from between the panel's changes,
        // never across one.
        let seen = self.seen();
        let _mutation = self.lock();
        let (rows, choice) = self.snapshot();
        let answer = self.resolve(&rows, &choice, &seen, profile)?;
        self.slot_of_answer(answer, &seen, true)
    }

    /// None also for a helper's model whose helper is away, and for any the regulator keeps in: the caller's profile
    /// answers, with its own way down.
    fn slot_of(&self, provider: &str, model: &str) -> Option<ModelSlot> {
        self.pick_of(provider, model).ok()
    }

    fn reachable(&self, profile: ModelProfile) -> bool {
        self.reachable_slot(profile).is_some()
    }

    fn now(&self, profile: ModelProfile) -> Option<BrainNow> {
        Some(BrainService::now(self, profile))
    }

    /// Every model that may take a round now, of the fast profile and of the smart one (the commands with tools): the
    /// one that answers, and the one behind it — a cloud backup behind a model at home takes the round that model
    /// fails, with what the earlier rounds read. The words leave nothing.
    fn in_cloud(&self) -> Option<bool> {
        let seen = self.seen();
        let (rows, choice) = self.snapshot();
        let out = |pick: &Pick| pick.reach != Reach::Home;
        let leaves = |profile| match self.resolve(&rows, &choice, &seen, profile) {
            Some(Answer::Chosen(chosen, Behind::Model(backup))) => out(&chosen) || out(&backup),
            Some(Answer::Chosen(answering, _) | Answer::Instead(answering, _)) => out(&answering),
            Some(Answer::Words(_)) | None => false,
        };
        Some(leaves(ModelProfile::Fast) || leaves(ModelProfile::Smart))
    }

    fn description(&self) -> String {
        let (rows, choice) = self.snapshot();
        let Some((fast_row, fast)) = Self::picked(&rows, &choice, ModelProfile::Fast) else {
            return self.env.description();
        };
        let second = match Self::picked(&rows, &choice, ModelProfile::Smart) {
            Some((row, pick)) if row.id != fast_row.id || pick.model != fast.model => {
                let via = if row.id == fast_row.id { String::new() } else { format!(" via {}", row.title) };
                format!(", smart {}{via}", pick.model)
            }
            _ => String::new(),
        };
        let effort = if fast_row.kind == ProviderKind::Anthropic {
            format!(", effort {}", self.effort_of(&choice))
        } else {
            String::new()
        };
        format!("{}{second} via {} (panel){effort}", fast.model, fast_row.title)
    }

    fn status(&self) -> BrainStatus {
        let (rows, choice) = self.snapshot();
        self.status_of(&rows, &choice)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use axum::extract::State as AxumState;
    use axum::http::{HeaderMap as Headers, Method, Uri};
    use axum::response::IntoResponse;

    /// What the fake service saw: the chats with their headers.
    #[derive(Clone, Default)]
    pub(crate) struct Service {
        pub chats: Arc<Mutex<Vec<(Value, Headers)>>>,
    }

    /// An OpenAI-compatible service: a model list (with the right key), words, and a tool call whenever tools are
    /// offered.
    async fn answer(
        AxumState(service): AxumState<Service>,
        method: Method,
        uri: Uri,
        headers: Headers,
        body: String,
    ) -> axum::response::Response {
        if method == Method::GET && uri.path().ends_with("/models") {
            if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer sk-test-key-1234") {
                return (axum::http::StatusCode::UNAUTHORIZED, r#"{"error":"bad key"}"#).into_response();
            }
            let list = json!({"data": [{"id": "small-model"}, {"id": "text-embedding-3"}, {"id": "big-model"}]});
            return ([("content-type", "application/json")], list.to_string()).into_response();
        }
        let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        service.chats.lock().unwrap().push((request.clone(), headers));
        let delta = match request["tools"].as_array().filter(|tools| !tools.is_empty()) {
            Some(tools) => {
                json!({"tool_calls": [{"index": 0, "id": "c1", "function": {"name": tools[0]["function"]["name"], "arguments": "{}"}}]})
            }
            None => json!({"content": "Да."}),
        };
        let stream = format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices": [{"delta": delta}]}));
        ([("content-type", "text/event-stream")], stream).into_response()
    }

    pub(crate) async fn service() -> (Service, String) {
        let service = Service::default();
        let app = axum::Router::new().fallback(answer).with_state(service.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (service, url)
    }

    fn dialog_config(extra: &[(&str, &str)]) -> DialogConfig {
        let mut pairs = vec![("DIALOG_PROVIDER", "none"), ("LLM_MAX_TOKENS", "300"), ("LLM_TIMEOUT_MS", "2000")];
        pairs.extend_from_slice(extra);
        Config::for_tests(&Env::from_pairs(pairs)).dialog
    }

    fn brain_with(config: DialogConfig) -> (BrainService, Arc<SecretsService>) {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        (BrainService::new(config, db, settings, secrets.clone(), events).unwrap(), secrets)
    }

    fn brain() -> (BrainService, Arc<SecretsService>) {
        brain_with(dialog_config(&[]))
    }

    fn input(value: Value) -> ProviderInput {
        ProviderInput::from_value(value)
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn a_backup_comes_with_the_chosen_model_and_the_env_model_may_be_it() {
        let (_service, url) = service().await;
        let (brain, _) = brain_with(dialog_config(&[
            ("DIALOG_PROVIDER", "openai"),
            ("LLM_MODEL", "env-model"),
            ("LLM_BASE_URL", url.as_str()),
        ]));
        let first = brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-test-key-1234"})), None)
            .unwrap();
        let second =
            brain.add(&input(json!({"preset": "custom-openai", "title": "Запас", "baseUrl": url})), None).unwrap();
        let choice = json!({
            "fast": {"provider": first.id, "model": "small-model"},
            "backup": {"provider": second.id, "model": "big-model"},
        });
        brain.choose(&object(choice), None).unwrap();
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        let backup = slot.backup.as_ref().expect("the backup comes with the chosen model");
        assert_eq!(
            (backup.slot.provider.as_str(), backup.slot.model.as_str(), backup.slot.title.as_str()),
            (second.id.as_str(), "big-model", "Запас")
        );
        assert!(backup.slot.backup.is_none() && !backup.taken());
        assert_eq!(
            brain.view().choice.backup,
            Some(BrainPick { provider: second.id.clone(), model: "big-model".into() })
        );
        assert!(brain.remove(&second.id, None).unwrap_err().message.contains("запасная"));

        // The chosen model itself is no backup.
        brain.choose(&object(json!({"backup": {"provider": first.id, "model": "small-model"}})), None).unwrap();
        assert!(brain.slot(ModelProfile::Fast).unwrap().backup.is_none());
        // The .env model may be the backup.
        brain.choose(&object(json!({"backup": {"provider": "env", "model": "env-model"}})), None).unwrap();
        assert_eq!(brain.slot(ModelProfile::Smart).unwrap().backup.unwrap().slot.provider, "env");
        assert!(brain.choose(&object(json!({"backup": {"provider": "nobody", "model": "m"}})), None).is_err());
        brain.choose(&object(json!({"backup": null})), None).unwrap();
        assert!(brain.slot(ModelProfile::Fast).unwrap().backup.is_none());
        assert_eq!(serde_json::to_value(brain.view().choice).unwrap().get("backup"), None);
        brain.remove(&second.id, None).unwrap();
    }

    #[tokio::test]
    async fn reads_the_models_the_chatgpt_plan_lists_by_slug() {
        let list = json!({"models": [
            {"slug": "gpt-plan", "visibility": "list", "display_name": "GPT"},
            {"slug": "gpt-hidden", "visibility": "hide"},
            {"slug": "gpt-old"},
        ]});
        let app = axum::Router::new()
            .route("/v1/models", axum::routing::get(move || std::future::ready(axum::Json(list.clone()))));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let (brain, _) = brain();
        let provider = brain.add(&input(json!({"preset": "custom-openai", "baseUrl": url})), None).unwrap();
        assert_eq!(brain.models(&provider.id).await.unwrap(), vec!["gpt-old", "gpt-plan"]);
    }

    #[tokio::test]
    async fn a_chatgpt_provider_shows_its_sign_in_and_takes_its_tokens_away_when_removed() {
        let (brain, secrets) = brain();
        let added = brain.add(&input(json!({"preset": "chatgpt"})), None).unwrap();
        assert_eq!(serde_json::to_value(&added.account).unwrap(), json!({"signedIn": false, "problem": "signin"}));
        assert!(!added.needs_key && !added.local);
        // Signed in once (no refresh token here: nothing to revoke at the service in a test).
        let record = TokenRecord {
            client_id: "oaiapp_1".into(),
            access_token: "access-1".into(),
            expires_at: crate::utils::time::now_ms() as i64 + 3_600_000,
            email: Some("owner@example.com".into()),
            ..TokenRecord::default()
        };
        let target = brain.sign_in_target("chatgpt").unwrap();
        let Target::Provider { id, created_at } = target;
        brain.signed_in(&id, &created_at, Credential::Tokens(record), None).unwrap();
        let view = serde_json::to_value(brain.view()).unwrap();
        let card = &view["providers"][0];
        assert_eq!(card["account"], json!({"signedIn": true, "who": "o…@example.com"}));
        assert_eq!(card["key"], json!({"set": false}));
        assert!(!view.to_string().contains("access-1"));
        // The client answers with the plan's API and the sign-in's token.
        let slot = brain.slot_of("chatgpt", "gpt-plan").unwrap();
        assert!(slot.client.description().ends_with("api.openai.com/v1/responses"), "{}", slot.client.description());
        brain.remove("chatgpt", None).unwrap();
        assert!(secrets.get("oauth:llm:chatgpt").is_none() && secrets.ids("llm:").is_empty());
    }

    #[tokio::test]
    async fn answers_with_env_until_the_panel_chooses_then_with_the_chosen_provider_from_the_next_question() {
        let (service, url) = service().await;
        let (brain, _) = brain();
        assert!(brain.slot(ModelProfile::Fast).is_none(), "DIALOG_PROVIDER=none");
        assert_eq!(brain.status().source, BrainOrigin::Env);
        let provider = brain
            .add(
                &input(json!({"preset": "custom-openai", "baseUrl": format!("{url}/"), "key": "sk-test-key-1234"})),
                None,
            )
            .unwrap();
        assert_eq!(provider.id, "custom-openai");
        assert_eq!(provider.base_url.as_deref(), Some(url.as_str()));
        assert_eq!(provider.key, SecretMask { set: true, hint: Some("…1234".into()) });
        assert!(!provider.cloud);
        assert!(!serde_json::to_string(&brain.view()).unwrap().contains("sk-test-key"));
        brain.choose(&object(json!({"fast": {"provider": provider.id, "model": "small-model"}})), None).unwrap();
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.model.as_str(), slot.provider.as_str(), slot.cloud), ("small-model", "custom-openai", false));
        assert_eq!(
            brain.slot(ModelProfile::Smart).unwrap().model,
            "small-model",
            "the smart profile follows the fast one"
        );
        // A model of its own apart from the profiles (the workshop agent's): that provider's, that model.
        let own = brain.slot_of(&provider.id, "big-model").unwrap();
        assert_eq!((own.model.as_str(), own.provider.as_str()), ("big-model", "custom-openai"));
        assert!(brain.slot_of("gone", "big-model").is_none(), "a provider that is gone: the caller's profile answers");
        let status = brain.status();
        assert_eq!(status.source, BrainOrigin::Panel);
        assert_eq!(status.models, BrainModels { fast: "small-model".into(), smart: "small-model".into() });
        assert_eq!(brain.description(), "small-model via Свой, OpenAI-совместимый (panel)");
        let request = LlmRequest {
            system: "s".into(),
            messages: vec![LlmMessage::User("q".into())],
            model: Some(slot.model.clone()),
            ..LlmRequest::default()
        };
        let _: Vec<_> = slot.client.stream(request, CancellationToken::new()).collect().await;
        let (_, headers) = service.chats.lock().unwrap().last().cloned().unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-test-key-1234");
    }

    #[tokio::test]
    async fn checks_a_provider_its_chat_models_the_time_to_the_first_word_whether_it_calls_tools() {
        let (_service, url) = service().await;
        let (brain, _) = brain();
        let provider = brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-test-key-1234"})), None)
            .unwrap();
        let check = brain.check(&provider.id, Some(&json!("big-model"))).await.unwrap();
        assert!(check.ok, "{check:?}");
        assert_eq!(check.model.as_deref(), Some("big-model"));
        assert_eq!(check.tools, Some(true));
        assert!(check.first_word_ms.is_some());
        assert_eq!(brain.view().providers[0].models, vec!["big-model", "small-model"]);
        assert_eq!(brain.view().providers[0].check.as_ref(), Some(&check));
        brain.update(&provider.id, &input(json!({"key": "sk-wrong-key-0000"})), None).unwrap();
        let refused = brain.models(&provider.id).await.unwrap_err();
        assert!(refused.to_string().contains("401"), "{refused}");
    }

    #[tokio::test]
    async fn keeps_a_provider_that_answers_now_and_takes_its_key_with_it_when_it_goes() {
        let (_service, url) = service().await;
        let (brain, secrets) = brain();
        let provider = brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-test-key-1234"})), None)
            .unwrap();
        brain.choose(&object(json!({"fast": {"provider": provider.id, "model": "small-model"}})), None).unwrap();
        let busy = brain.remove(&provider.id, None).unwrap_err();
        assert!(busy.message.contains("сейчас отвечает"), "{busy}");
        brain.choose(&object(json!({"fast": null})), None).unwrap();
        assert_eq!(brain.status().source, BrainOrigin::Env);
        brain.remove(&provider.id, None).unwrap();
        assert!(secrets.ids("llm:").is_empty());
        assert!(brain.view().providers.is_empty());
    }

    #[tokio::test]
    async fn addresses_yandexgpt_models_through_the_folder_and_shows_them_short() {
        let (brain, _) = brain();
        let provider = brain
            .add(&input(json!({"preset": "yandexgpt", "key": "AQVN-test-key-5678", "options": {"folder": "b1gfolder", "extra": "x"}})), None)
            .unwrap();
        assert_eq!(provider.options, BTreeMap::from([("folder".to_string(), "b1gfolder".to_string())]));
        assert_eq!(provider.base_url.as_deref(), Some("https://llm.api.cloud.yandex.net/v1"));
        brain.choose(&object(json!({"fast": {"provider": provider.id, "model": "yandexgpt/latest"}})), None).unwrap();
        assert_eq!(brain.slot(ModelProfile::Fast).unwrap().model, "gpt://b1gfolder/yandexgpt/latest");
        assert_eq!(brain.status().models.fast, "yandexgpt/latest");
    }

    #[tokio::test]
    async fn refuses_what_it_cannot_use() {
        let (brain, _) = brain();
        assert!(brain.add(&input(json!({"preset": "nope"})), None).unwrap_err().message.contains("Неизвестный"));
        assert!(brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": "ftp://x"})), None)
            .unwrap_err()
            .message
            .contains("http"));
        assert!(brain
            .add(&input(json!({"preset": "custom-openai"})), None)
            .unwrap_err()
            .message
            .contains("Укажите адрес"));
        let missing = brain.choose(&object(json!({"fast": {"provider": "missing", "model": "m"}})), None).unwrap_err();
        assert!(missing.message.contains("нет"), "{missing}");
        assert!(brain.choose(&object(json!({"effort": "extreme"})), None).unwrap_err().message.contains("Глубина"));
        assert_eq!(
            brain.update("missing", &input(json!({})), None).unwrap_err().status,
            axum::http::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn a_claude_provider_without_its_own_key_sends_nothing_though_the_environment_has_one() {
        let (brain, _) = brain_with(dialog_config(&[("ANTHROPIC_API_KEY", "environment-key")]));
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = hits.clone();
        let app = axum::Router::new().fallback(move |headers: Headers| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(format!("{headers:?}"));
                axum::http::StatusCode::NOT_FOUND
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let provider = brain.add(&input(json!({"preset": "custom-anthropic", "baseUrl": url})), None).unwrap();
        let check = brain.check(&provider.id, Some(&json!("claude-haiku-5-5"))).await.unwrap();
        assert_eq!(check.problem.as_deref(), Some("Нет ключа."), "{check:?}");
        assert!(hits.lock().unwrap().is_empty(), "nothing went out");
    }

    #[test]
    fn says_what_went_wrong_in_the_owners_words_and_orders_models_as_people_read_them() {
        let (brain, secrets) = brain();
        secrets.set("llm:x:key", Some("sk-secret-value-1")).unwrap();
        let problem = |text: &str| brain.problem_of(&anyhow!("{text}"));
        assert_eq!(problem("no API key: add it to the provider in the panel"), "Нет ключа.");
        assert_eq!(problem("Anthropic rejected the API key"), "Ключ не принят.");
        assert_eq!(problem("HTTP 401: nope"), "Ключ не принят.");
        assert_eq!(problem("Anthropic does not know the model: 404 {}"), "Модель или адрес не найдены.");
        assert_eq!(problem("HTTP 429: slow down"), "Сервис отказал: лимит или пустой счёт.");
        assert_eq!(problem("cannot reach http://x/v1/models: connection refused"), "Адрес не отвечает.");
        assert_eq!(problem("Claude сейчас недоступен: нет связи с Anthropic."), "Адрес не отвечает.");
        assert_eq!(problem("Claude is unavailable now: cannot reach Anthropic."), "Адрес не отвечает.");
        assert_eq!(problem("Claude сейчас недоступен: Anthropic не ответил вовремя."), "Не ответил за 20 секунд.");
        assert_eq!(problem("Claude is unavailable now: Anthropic did not answer in time."), "Не ответил за 20 секунд.");
        assert_eq!(problem("Request was aborted."), "Не ответил за 20 секунд.");
        assert_eq!(problem("odd sk-secret-value-1"), "Ошибка: odd •••");
        let mut models =
            vec!["b-model".to_string(), "Qwen/Qwen3".into(), "meta/llama".into(), "a_model".into(), "a-model".into()];
        models.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(models, vec!["a_model", "a-model", "b-model", "meta/llama", "Qwen/Qwen3"]);
    }

    #[test]
    fn a_key_never_outlives_its_provider_to_reach_the_next_one_under_its_id() {
        let (brain, secrets) = brain();
        let home = json!({"preset": "custom-openai", "baseUrl": "http://10.0.0.5:8080/v1", "key": "sk-first-key-1111"});
        let first = brain.add(&input(home), None).unwrap();
        // Gone between what this service holds and the table (another request, a removal that failed half-way):
        // the row is not there, its key and the held row are.
        brain.db.with(|c| c.execute("DELETE FROM llm_providers WHERE id = ?1", params![first.id])).unwrap();
        let gone = brain.update(&first.id, &input(json!({"key": "sk-moved-key-2222"})), None).unwrap_err();
        assert_eq!(gone.status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            secrets.get("llm:custom-openai:key").as_deref(),
            Some("sk-first-key-1111"),
            "an update of nothing writes no key"
        );
        // The next provider takes the id back, never the key left under it.
        let next =
            brain.add(&input(json!({"preset": "custom-openai", "baseUrl": "http://10.0.0.6:8080/v1"})), None).unwrap();
        assert_eq!(next.id, first.id);
        assert_eq!(next.key, SecretMask { set: false, hint: None });
        assert!(secrets.ids("llm:").is_empty());
    }

    #[test]
    fn an_update_writes_only_what_it_names() {
        let (brain, _) = brain();
        let home = json!({"preset": "custom-openai", "title": "Дома", "baseUrl": "http://10.0.0.5:8080/v1"});
        let provider = brain.add(&input(home), None).unwrap();
        // Another change landed after the held rows were read.
        brain
            .db
            .with(|c| {
                c.execute(
                    "UPDATE llm_providers SET title = 'Дача', base_url = 'http://10.0.0.9:8080/v1' WHERE id = ?1",
                    params![provider.id],
                )
            })
            .unwrap();
        let keyed = brain.update(&provider.id, &input(json!({"key": "sk-home-key-3333"})), None).unwrap();
        assert_eq!(
            (keyed.title.as_str(), keyed.base_url.as_deref()),
            ("Дача", Some("http://10.0.0.9:8080/v1")),
            "a key changes neither the title nor the address"
        );
        assert_eq!(keyed.key.hint.as_deref(), Some("…3333"));
        let moved = brain.update(&provider.id, &input(json!({"baseUrl": "http://10.0.0.7:8080/v1"})), None).unwrap();
        assert_eq!((moved.title.as_str(), moved.base_url.as_deref()), ("Дача", Some("http://10.0.0.7:8080/v1")));
        let renamed = brain.update(&provider.id, &input(json!({"title": null})), None).unwrap();
        assert_eq!(renamed.title, "Свой, OpenAI-совместимый", "a title given empty is the preset's");
        assert_eq!(renamed.base_url.as_deref(), Some("http://10.0.0.7:8080/v1"));
    }

    #[test]
    fn concurrent_changes_leave_no_key_without_its_provider() {
        let (brain, secrets) = brain();
        let brain = Arc::new(brain);
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let brain = brain.clone();
                std::thread::spawn(move || {
                    for round in 0..25 {
                        if worker % 2 == 0 {
                            let _ = brain.remove("custom-openai", None);
                            let url = format!("http://10.0.{worker}.{round}:8080/v1");
                            let key = format!("sk-added-{worker}-{round}");
                            let _ =
                                brain.add(&input(json!({"preset": "custom-openai", "baseUrl": url, "key": key})), None);
                        } else {
                            let key = format!("sk-updated-{worker}-{round}");
                            let _ = brain.update("custom-openai", &input(json!({"key": key})), None);
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let mut ids = brain.ids().unwrap();
        for secret in secrets.ids("llm:") {
            let owner = secret.trim_start_matches("llm:").trim_end_matches(":key");
            assert!(ids.iter().any(|id| id == owner), "{secret} outlived its provider: {ids:?}");
        }
        let mut held: Vec<String> = brain.view().providers.into_iter().map(|provider| provider.id).collect();
        held.sort();
        ids.sort();
        assert_eq!(held, ids, "what is held is what is stored");
    }

    #[test]
    fn keeps_the_providers_it_holds_when_they_cannot_be_read_and_says_so() {
        use axum::http::StatusCode;
        let (brain, _) = brain();
        let home = json!({"preset": "custom-openai", "baseUrl": "http://10.0.0.5:8080/v1", "key": "sk-home-key-1234"});
        let provider = brain.add(&input(home), None).unwrap();
        brain.choose(&object(json!({"fast": {"provider": provider.id, "model": "home-model"}})), None).unwrap();
        let rename = |from: &str, to: &str| {
            brain.db.with(|c| c.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))).unwrap();
        };
        rename("llm_providers", "llm_providers_away");
        assert_eq!(brain.reload().unwrap_err().status, StatusCode::INTERNAL_SERVER_ERROR);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), ("custom-openai", "home-model"), "not .env");
        assert_eq!(brain.status().source, BrainOrigin::Panel);
        let update = brain.update(&provider.id, &input(json!({"title": "Дача"})), None).unwrap_err();
        assert_eq!(update.status, StatusCode::INTERNAL_SERVER_ERROR);
        let add = brain.add(&input(json!({"preset": "ollama"})), None).unwrap_err();
        assert_eq!(add.status, StatusCode::INTERNAL_SERVER_ERROR);
        rename("llm_providers_away", "llm_providers");
        // A choice that cannot be read is an error too, never «nothing chosen».
        rename("settings", "settings_away");
        assert!(brain.reload().is_err());
        assert_eq!(brain.status().source, BrainOrigin::Panel);
        rename("settings_away", "settings");
        brain.reload().unwrap();
        assert_eq!(brain.slot(ModelProfile::Fast).unwrap().provider, "custom-openai");
    }

    #[test]
    fn does_not_start_on_providers_it_cannot_read() {
        let db = Arc::new(Db::memory());
        db.with(|c| c.execute_batch("ALTER TABLE llm_providers RENAME TO llm_providers_away")).unwrap();
        let events = EventBus::new();
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        assert!(BrainService::new(dialog_config(&[]), db, settings, secrets, events).is_err());
    }

    // The helpers' models (docs/plans/home-and-helpers.md, stage 4).

    /// The helpers as a test sets them: what «Мозг» reads of them, and the reaches the regulator keeps in.
    #[derive(Default)]
    pub(crate) struct FakeHelpers {
        pub list: Mutex<Vec<ModelHelper>>,
        pub shut: Mutex<Vec<Reach>>,
    }

    impl FakeHelpers {
        pub(crate) fn set(&self, list: Vec<ModelHelper>) {
            *self.list.lock().unwrap() = list;
        }

        pub(crate) fn up(&self, id: &str, up: bool) {
            for helper in self.list.lock().unwrap().iter_mut().filter(|helper| helper.id == id) {
                helper.up = up;
                helper.state = if up { HelperState::Up } else { HelperState::Down };
            }
        }

        pub(crate) fn shut(&self, reaches: &[Reach]) {
            *self.shut.lock().unwrap() = reaches.to_vec();
        }
    }

    impl ModelHelpers for FakeHelpers {
        fn model_helpers(&self) -> Vec<ModelHelper> {
            self.list.lock().unwrap().clone()
        }

        fn lets(&self, reach: Reach) -> bool {
            !self.shut.lock().unwrap().contains(&reach)
        }
    }

    /// A plain helper at home whose model is asked at `url` with the fake service's key; read, on the line or not. A
    /// helper on the line has its list asked in the background: only a stand-in on 127.0.0.1 is ever on the line.
    pub(crate) fn model_helper(id: &str, name: &str, url: &str, up: bool) -> ModelHelper {
        ModelHelper {
            id: id.into(),
            name: name.into(),
            kind: HelperKind::Home,
            base_url: url.into(),
            key: Some("sk-test-key-1234".into()),
            reach: Reach::Home,
            up,
            state: if up { HelperState::Up } else { HelperState::Down },
            via: None,
            read: true,
            linked: false,
            provider: None,
            builtin: false,
        }
    }

    /// «Мозг» knowing the test's helpers.
    pub(crate) fn with_helpers() -> (Arc<BrainService>, Arc<FakeHelpers>, Arc<SecretsService>) {
        let (brain, secrets) = brain();
        let helpers = Arc::new(FakeHelpers::default());
        brain.set_helpers(helpers.clone());
        (Arc::new(brain), helpers, secrets)
    }

    /// The row of a helper's model.
    fn row_of(brain: &BrainService, helper: &str) -> Option<ProviderView> {
        brain.view().providers.into_iter().find(|row| row.helper.as_ref().is_some_and(|whose| whose.id == helper))
    }

    fn choose(brain: &BrainService, choice: Value) {
        brain.choose(&object(choice), None).unwrap();
    }

    /// A provider's address as the table stores it: what an earlier version reads.
    fn stored_url(brain: &BrainService, id: &str) -> Option<String> {
        brain
            .db
            .with(|c| c.query_row("SELECT base_url FROM llm_providers WHERE id = ?1", [id], |row| row.get(0)))
            .unwrap()
    }

    /// A model's list as «Обновить» would leave it.
    fn listed(brain: &BrainService, id: &str, models: &[&str]) {
        brain
            .db
            .with(|c| {
                c.execute("UPDATE llm_providers SET models = ?2 WHERE id = ?1", params![id, json!(models).to_string()])
            })
            .unwrap();
        brain.reload().unwrap();
    }

    fn question() -> LlmRequest {
        LlmRequest { system: "s".into(), messages: vec![LlmMessage::User("q".into())], ..LlmRequest::default() }
    }

    /// What a slot's client says: its words when it fails with them.
    async fn said(slot: &ModelSlot) -> Result<String, String> {
        let request = LlmRequest { model: Some(slot.model.clone()), ..question() };
        let mut text = String::new();
        let mut events = slot.client.stream(request, CancellationToken::new());
        while let Some(event) = events.next().await {
            match event {
                Ok(LlmEvent::Text(piece)) => text.push_str(&piece),
                Ok(_) => {}
                Err(error) => {
                    return Err(error.downcast_ref::<SpokenFailure>().map(|f| f.phrase.clone()).unwrap_or_default())
                }
            }
        }
        Ok(text)
    }

    /// The `brain.changed` events of these kinds seen so far.
    fn changes(events: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>, change: &str) -> usize {
        let mut count = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(&*event, GatewayEvent::BrainChanged { change: seen, .. } if seen == change) {
                count += 1;
            }
        }
        count
    }

    #[tokio::test]
    async fn each_helper_with_a_model_has_its_provider_and_the_old_one_takes_its_name() {
        let (brain, helpers, secrets) = with_helpers();
        let route = "http://192.168.1.20:7780/home/v1/llm";
        let old = brain
            .add(
                &input(json!({"preset": "custom-openai", "title": "Модель на компьютере", "baseUrl": route, "key": "vgw_old"})),
                None,
            )
            .unwrap();
        choose(&brain, json!({"fast": {"provider": old.id, "model": "home"}}));
        let mut events = brain.events.subscribe();
        // At home, not on the line: nothing is asked of it here.
        helpers.set(vec![ModelHelper {
            key: Some("vgw_home".into()),
            ..model_helper("home-computer", "Компьютер дома", route, false)
        }]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").expect("the old row is the computer's now");
        assert_eq!((row.id.as_str(), row.preset.as_str()), (old.id.as_str(), "helper"), "the same row, its id kept");
        assert_eq!(row.title, "Модель: Компьютер дома");
        assert_eq!(row.base_url.as_deref(), Some(route), "today's address: a station rolled back still reaches it");
        assert_eq!(secrets.get(&format!("llm:{}:key", old.id)).as_deref(), Some("vgw_home"), "the helper's key now");
        assert_eq!(
            row.options,
            BTreeMap::from([("helper".into(), "home-computer".into()), ("reach".into(), "home".into())])
        );
        assert_eq!(brain.view().choice.fast.unwrap().provider, old.id, "the choice points at it still");
        assert_eq!(changes(&mut events, "helper.model"), 1);

        // A linked helper beside it: a row of its own, with neither its bridge nor its token stored.
        let bridge = ModelHelper {
            kind: HelperKind::Own,
            reach: Reach::Mine,
            linked: true,
            key: Some("bridge-token".into()),
            ..model_helper("own-1", "Мой сервер", "http://127.0.0.1:7800/via/own-1/home/v1/llm", false)
        };
        helpers.set(vec![helpers.model_helpers()[0].clone(), bridge]);
        brain.keep_helper_models().unwrap();
        let linked = row_of(&brain, "own-1").expect("its own row");
        assert_eq!((linked.title.as_str(), linked.base_url.as_deref()), ("Модель: Мой сервер", None));
        assert_eq!(stored_url(&brain, &linked.id).as_deref(), Some(HELPER_NO_ADDRESS), "the stand-in in the table");
        assert!(!linked.key.set && secrets.get(&format!("llm:{}:key", linked.id)).is_none());
        assert!(linked.cloud, "one's own server is out of the house");
        assert_eq!(linked.helper.as_ref().unwrap().kind, HelperKind::Own);
        // Nothing changed: nothing written, no event.
        brain.keep_helper_models().unwrap();
        assert_eq!(changes(&mut events, "helper.model"), 1);
    }

    /// The rollback risk of the helpers' rows (docs/plans/home-and-helpers.md, stage 4): a version from before the
    /// helpers (0.14.0, 83b585db: `settings_of` asks the row's address, else its preset's, else OpenAI's) knows no
    /// preset `helper`, so a row without an address would send a chosen question to api.openai.com. A row an earlier
    /// build of the helpers stored so takes the stand-in at the start; this version reads it as none and asks a helper's
    /// row without an address nowhere — at the stand-in, a closed port on the device itself.
    #[test]
    fn a_helpers_row_without_an_address_takes_the_stand_in_never_openais() {
        let (brain, _) = brain();
        let options = json!({"helper": "own-1", "reach": "mine"}).to_string();
        brain
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, created_at, updated_at)
                     VALUES ('helper-own-1', 'helper', 'openai', 'Модель: Мой сервер', NULL, ?1, 'now', 'now')",
                    params![options],
                )
            })
            .unwrap();
        let again = BrainService::new(
            brain.config.clone(),
            brain.db.clone(),
            brain.settings.clone(),
            brain.secrets.clone(),
            brain.events.clone(),
        )
        .unwrap();
        assert_eq!(stored_url(&again, "helper-own-1").as_deref(), Some(HELPER_NO_ADDRESS), "what 0.14.0 would ask");
        let url = url::Url::parse(HELPER_NO_ADDRESS).unwrap();
        assert_eq!((url.host_str(), url.port()), (Some("127.0.0.1"), Some(9)), "a closed port on the device itself");
        let (rows, _) = again.snapshot();
        let row = rows.iter().find(|row| row.id == "helper-own-1").unwrap();
        assert_eq!(row.base_url, None, "here the stand-in is no address");
        // Asked before the helpers are known, or with its helper gone: at the stand-in, never at OpenAI's.
        for seen in [Seen::default(), Seen { known: true, ..Seen::default() }] {
            assert_eq!(again.settings_of(row, &seen).base_url.as_deref(), Some(HELPER_NO_ADDRESS));
        }
    }

    #[tokio::test]
    async fn a_helper_renamed_moved_or_gone_takes_its_provider_along_unless_it_answers() {
        let (brain, helpers, secrets) = with_helpers();
        helpers.set(vec![model_helper("home-a", "Кабинет", "http://192.168.1.20:7780/home/v1/llm", false)]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-a").unwrap();
        assert_eq!(row.title, "Модель: Кабинет");

        helpers.set(vec![ModelHelper {
            key: Some("vgw_new_key".into()),
            ..model_helper("home-a", "Мастерская", "http://192.168.1.21:7780/home/v1/llm", false)
        }]);
        brain.keep_helper_models().unwrap();
        let moved = row_of(&brain, "home-a").unwrap();
        assert_eq!(moved.id, row.id);
        assert_eq!(moved.title, "Модель: Мастерская");
        assert_eq!(moved.base_url.as_deref(), Some("http://192.168.1.21:7780/home/v1/llm"));
        assert_eq!(secrets.get(&format!("llm:{}:key", row.id)).as_deref(), Some("vgw_new_key"));

        // Gone while a choice answers with it: kept, said off, its name as it was — without the gone helper's address
        // and key, which would travel on in every package.
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        helpers.set(Vec::new());
        brain.keep_helper_models().unwrap();
        let kept = row_of(&brain, "home-a").expect("kept while it answers");
        assert_eq!((kept.title.as_str(), kept.helper.unwrap().state), ("Модель: Мастерская", HelperState::Off));
        assert_eq!((kept.base_url, kept.key.set), (None, false));
        assert_eq!(stored_url(&brain, &row.id).as_deref(), Some(HELPER_NO_ADDRESS), "the gone helper's address goes");
        assert!(secrets.ids("llm:").is_empty());
        // Its helper gone, it is the person's to remove like any other: not while it answers.
        let refused = brain.remove(&row.id, None).unwrap_err();
        assert_eq!(refused.message, "Этот провайдер сейчас отвечает. Сначала выберите другую модель.");
        // Another model answers: it goes.
        choose(&brain, json!({"fast": null}));
        brain.keep_helper_models().unwrap();
        assert!(row_of(&brain, "home-a").is_none());
        // Back in the list: its row comes back under its id, with its address and key.
        helpers.set(vec![ModelHelper {
            key: Some("vgw_new_key".into()),
            ..model_helper("home-a", "Мастерская", "http://192.168.1.21:7780/home/v1/llm", false)
        }]);
        brain.keep_helper_models().unwrap();
        let back = row_of(&brain, "home-a").unwrap();
        assert_eq!((back.id.as_str(), back.key.set), (row.id.as_str(), true));
        // Gone again and kept by nothing but the person's «Удалить»: it goes.
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        helpers.set(Vec::new());
        brain.keep_helper_models().unwrap();
        choose(&brain, json!({"fast": null}));
        brain.remove(&row.id, None).unwrap();
        assert!(row_of(&brain, "home-a").is_none());
        assert!(secrets.ids("llm:").is_empty());
        // A helper not read yet gets no row, and keeps the one it has.
        helpers.set(vec![ModelHelper {
            read: false,
            ..model_helper("home-b", "Б", "http://192.168.1.22:7780/home/v1/llm", false)
        }]);
        brain.keep_helper_models().unwrap();
        assert!(row_of(&brain, "home-b").is_none(), "not before its first look says it has one");
    }

    #[tokio::test]
    async fn a_chosen_model_at_a_helper_that_is_away_gives_way_to_the_backup_at_once() {
        let (at_helper, helper_url) = service().await;
        let (at_spare, spare_url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper("home-computer", "Компьютер дома", &helper_url, true)]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        let spare = brain
            .add(&input(json!({"preset": "custom-openai", "title": "Запас", "baseUrl": spare_url, "key": "sk-test-key-1234"})), None)
            .unwrap();
        choose(
            &brain,
            json!({"fast": {"provider": row.id, "model": "small-model"}, "backup": {"provider": spare.id, "model": "big-model"}}),
        );
        // On the line: its model answers, the backup behind it.
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.title.as_str()), (row.id.as_str(), "Модель: Компьютер дома"));
        assert_eq!(slot.backup.as_ref().unwrap().slot.provider, spare.id);
        assert_eq!(brain.now(ModelProfile::Fast).state, Answering::Ready);

        // Away: the backup answers at once, nothing behind it; not a request goes to the helper.
        helpers.up("home-computer", false);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), (spare.id.as_str(), "big-model"));
        assert!(slot.backup.is_none());
        assert_eq!(said(&slot).await.as_deref(), Ok("Да."));
        assert_eq!((at_helper.chats.lock().unwrap().len(), at_spare.chats.lock().unwrap().len()), (0, 1));
        let now = brain.now(ModelProfile::Fast);
        assert_eq!(
            (now.state, now.provider.as_deref(), now.title.as_deref(), now.reason.as_deref()),
            (Answering::Fallback, Some(spare.id.as_str()), Some("Запас"), Some("Компьютер дома не на связи"))
        );
        assert!(BrainSource::reachable(&*brain, ModelProfile::Fast));
    }

    #[tokio::test]
    async fn without_a_backup_the_next_helper_with_a_model_answers() {
        let (at_b, b_url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![
            model_helper("home-a", "А", "http://192.168.1.20:7780/home/v1/llm", false),
            // On the line, but no model known for it: passed by.
            model_helper("home-c", "В", "http://127.0.0.1:9/v1", false),
            model_helper("home-b", "Б", &b_url, true),
        ]);
        brain.keep_helper_models().unwrap();
        helpers.up("home-c", true);
        let (a, b) = (row_of(&brain, "home-a").unwrap().id, row_of(&brain, "home-b").unwrap().id);
        listed(&brain, &b, &["big-model", "small-model"]);
        choose(&brain, json!({"fast": {"provider": a, "model": "small-model"}}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(
            (slot.provider.as_str(), slot.model.as_str()),
            (b.as_str(), "small-model"),
            "the chosen model, it has it"
        );
        assert!(slot.backup.is_none());
        assert_eq!(said(&slot).await.as_deref(), Ok("Да."));
        assert_eq!(at_b.chats.lock().unwrap().len(), 1);
        choose(&brain, json!({"fast": {"provider": a, "model": "home"}}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), (b.as_str(), "big-model"), "else the first it lists");
        let now = brain.now(ModelProfile::Fast);
        assert_eq!(
            (now.state, now.helper.as_deref(), now.reason.as_deref()),
            (Answering::Fallback, Some("home-b"), Some("А не на связи"))
        );
    }

    #[tokio::test]
    async fn without_a_backup_or_a_helper_the_words_come_at_once() {
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper(
            "home-computer",
            "Компьютер дома",
            "http://192.168.1.20:7780/home/v1/llm",
            false,
        )]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.title.as_str(), slot.cloud), ("words", "нет модели на связи", false));
        assert!(slot.backup.is_none());
        let words = "Компьютер дома не на связи, а запасной модели нет.";
        let events: Vec<_> = tokio::time::timeout(
            Duration::from_secs(1),
            slot.client.stream(question(), CancellationToken::new()).collect::<Vec<_>>(),
        )
        .await
        .expect("at once, not after a connection's ten seconds");
        assert_eq!(events.len(), 1);
        let failure = events.into_iter().next().unwrap().unwrap_err();
        assert_eq!(failure.downcast_ref::<SpokenFailure>().unwrap().phrase, words);
        assert!(
            !BrainSource::reachable(&*brain, ModelProfile::Fast)
                && !BrainSource::reachable(&*brain, ModelProfile::Smart)
        );
        assert!(brain.reachable_slot(ModelProfile::Fast).is_none());
        let now = brain.now(ModelProfile::Fast);
        assert_eq!((now.state, now.provider, now.reason.as_deref()), (Answering::Off, None, Some(words)));
        assert!(BrainSource::slot_of(&*brain, &row.id, "home").is_none(), "the workshop's own pick falls back too");
    }

    /// «Выход в интернет» (src/egress) refuses a cloud now — its VPN does not answer, or no connection fits —: it stands
    /// in for nobody (the merge with main, review of 16.11). The gateway does not take it as its own backup; a backup
    /// the person chose does not answer in the away helper's place: the next helper with a model does, else the words
    /// at once — and no model may be asked, so the station in «Вместе» gives the question to Alice rather than say
    /// «VPN не отвечает». Its VPN up again, it stands in again.
    #[tokio::test]
    async fn a_cloud_whose_vpn_does_not_answer_stands_in_for_nobody() {
        use std::sync::atomic::{AtomicBool, Ordering as Atomic};
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![
            model_helper("home-computer", "Компьютер дома", "http://192.0.2.20:7780/home/v1/llm", false),
            model_helper("home-b", "Кабинет", "http://192.0.2.21:7780/home/v1/llm", false),
        ]);
        brain.keep_helper_models().unwrap();
        let (row, other) = (row_of(&brain, "home-computer").unwrap(), row_of(&brain, "home-b").unwrap());
        listed(&brain, &other.id, &["home"]);
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        let down = Arc::new(AtomicBool::new(true));
        {
            let (down, claude) = (down.clone(), claude.id.clone());
            brain.set_refusals(Arc::new(move |provider| provider == claude && down.load(Atomic::SeqCst)));
        }
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.note.unwrap().kind), (None, "backupNeeded"), "not the refused cloud");
        down.store(false, Atomic::SeqCst);
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup.map(|pick| pick.provider), Some(claude.id.clone()), "its VPN answers");

        // The computer away, the backup's VPN silent: the words at once, no model to ask.
        down.store(true, Atomic::SeqCst);
        let words = "Компьютер дома не на связи, а запасная модель недоступна: VPN не отвечает.";
        let now = brain.now(ModelProfile::Fast);
        assert_eq!((now.state, now.provider, now.reason.as_deref()), (Answering::Off, None, Some(words)));
        assert!(!BrainSource::reachable(&*brain, ModelProfile::Fast), "the station gives the question to Alice");
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err(words.to_string()));
        assert!(!BrainSource::in_cloud(&*brain).unwrap(), "the words leave nothing");

        // Another helper with a model on the line: it answers in the computer's place.
        helpers.up("home-b", true);
        let now = brain.now(ModelProfile::Fast);
        assert_eq!((now.state, now.helper.as_deref()), (Answering::Fallback, Some("home-b")));
        helpers.up("home-b", false);

        // The VPN answers again: the backup at once.
        down.store(false, Atomic::SeqCst);
        let now = brain.now(ModelProfile::Fast);
        assert_eq!(
            (now.state, now.provider.as_deref(), now.reason.as_deref()),
            (Answering::Fallback, Some(claude.id.as_str()), Some("Компьютер дома не на связи"))
        );
        assert!(BrainSource::reachable(&*brain, ModelProfile::Fast));
    }

    #[tokio::test]
    async fn the_regulator_shuts_a_cloud_backup_and_the_words_say_so() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper(
            "home-computer",
            "Компьютер дома",
            "http://192.168.1.20:7780/home/v1/llm",
            false,
        )]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        let local = brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-test-key-1234"})), None)
            .unwrap();
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        helpers.shut(&[Reach::Mine, Reach::Foreign]);
        let words_of = |slot: &ModelSlot| (slot.provider == "words").then(|| slot.client.clone());
        // The main model at home and on the line: words behind it, not the cloud.
        choose(
            &brain,
            json!({"fast": {"provider": local.id, "model": "small-model"}, "backup": {"provider": claude.id, "model": "claude-haiku-5-5"}}),
        );
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(slot.provider, local.id);
        let behind = slot.backup.as_ref().map(|backup| backup.slot.clone()).unwrap();
        assert!(words_of(&behind).is_some());
        assert_eq!(
            said(&behind).await,
            Err("Модель не ответила, а облака сейчас закрыты — запасная модель не ответит.".into())
        );
        // The main model's helper away: the words at once.
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(
            said(&slot).await,
            Err("Компьютер дома не на связи, а облака сейчас закрыты — запасная модель не ответит.".into())
        );
        // The main model a cloud: never asked.
        choose(&brain, json!({"fast": {"provider": claude.id, "model": "claude-haiku-5-5"}, "backup": null}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err("Модель в облаке, а облака сейчас закрыты.".into()));
        assert_eq!(brain.now(ModelProfile::Fast).state, Answering::Off);
        // The clouds let out again: Claude answers, nothing behind it.
        helpers.shut(&[]);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.backup.is_none()), (claude.id.as_str(), true));
    }

    #[tokio::test]
    async fn a_model_at_a_helper_on_the_line_carries_words_when_nothing_else_could_answer() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper("home-computer", "Компьютер дома", &url, true)]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        choose(&brain, json!({"fast": {"provider": row.id, "model": "small-model"}}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(slot.provider, row.id);
        let behind = slot.backup.as_ref().unwrap().slot.clone();
        assert_eq!(said(&behind).await, Err("Компьютер дома: модель не ответила, а запасной нет.".into()));
        assert!(BrainSource::reachable(&*brain, ModelProfile::Fast));
        assert!(brain.reachable_slot(ModelProfile::Fast).unwrap().backup.is_none(), "words are no model to reach");
        assert_eq!(brain.now(ModelProfile::Fast).helper.as_deref(), Some("home-computer"));
    }

    /// W6 and W3: a model at home on the line carries words behind it while its backup's helper is away; with both
    /// helpers away the words name them both.
    #[tokio::test]
    async fn the_words_name_the_backups_helper_away_behind_or_beside_the_chosen_one() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![
            model_helper("home-a", "Кабинет", &url, true),
            model_helper("home-b", "Мастерская", "http://192.168.1.21:7780/home/v1/llm", false),
        ]);
        brain.keep_helper_models().unwrap();
        let (a, b) = (row_of(&brain, "home-a").unwrap().id, row_of(&brain, "home-b").unwrap().id);
        choose(
            &brain,
            json!({"fast": {"provider": a, "model": "small-model"}, "backup": {"provider": b, "model": "big-model"}}),
        );
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(slot.provider, a);
        let behind = slot.backup.as_ref().unwrap().slot.clone();
        assert_eq!(said(&behind).await, Err("Модель не ответила, а Мастерская с запасной моделью не на связи.".into()));

        helpers.up("home-a", false);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err("Ни Кабинет, ни Мастерская сейчас не на связи.".into()));
        assert_eq!(brain.now(ModelProfile::Fast).state, Answering::Off);
    }

    /// A backup on the chosen model's own helper — a small model there for a big one that fails to load — stands
    /// behind it while the helper is on the line, and is no backup while the helper is away: W1, not «neither A nor A»;
    /// «Мозг» asks for another, and the gateway never puts its own in place of the person's.
    #[tokio::test]
    async fn a_backup_on_the_chosen_models_own_helper_is_away_with_it() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper("home-computer", "Компьютер дома", &url, true)]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap().id;
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        choose(
            &brain,
            json!({"fast": {"provider": row, "model": "qwen3:8b"}, "backup": {"provider": row, "model": "qwen3:1.7b"}}),
        );
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        let behind = &slot.backup.as_ref().unwrap().slot;
        assert_eq!((behind.provider.as_str(), behind.model.as_str()), (row.as_str(), "qwen3:1.7b"));
        assert_eq!(brain.view().note.map(|note| note.kind), Some("backupNeeded"), "it is away with the main one");

        helpers.up("home-computer", false);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err("Компьютер дома не на связи, а запасной модели нет.".into()));
        assert_eq!(
            brain.now(ModelProfile::Fast).reason.as_deref(),
            Some("Компьютер дома не на связи, а запасной модели нет.")
        );
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup.unwrap().model, "qwen3:1.7b", "the person's backup stays");
    }

    /// A helper's row has its helper's id (`helper-<entry>`): a pick that outlives its row — the memory's, the
    /// workshop's, an app's — never passes to another helper's model whose row came later, and finds its own helper's
    /// row again when that helper comes back.
    #[tokio::test]
    async fn a_helpers_row_has_its_helpers_id_and_never_passes_to_another_helper() {
        let (brain, helpers, _) = with_helpers();
        let a = model_helper("home-a", "Кабинет", "http://192.168.1.20:7780/home/v1/llm", false);
        helpers.set(vec![a.clone()]);
        brain.keep_helper_models().unwrap();
        assert_eq!(row_of(&brain, "home-a").unwrap().id, "helper-home-a");
        // Switched off — nothing in «Мозг» answers with it; a memory's pick does not keep it —, then another added.
        helpers.set(Vec::new());
        brain.keep_helper_models().unwrap();
        helpers.set(vec![model_helper("own-b", "Сервер", "http://192.168.1.21:7780/home/v1/llm", false)]);
        brain.keep_helper_models().unwrap();
        assert_eq!(row_of(&brain, "own-b").unwrap().id, "helper-own-b");
        assert_eq!(brain.pick_of("helper-home-a", "qwen2.5").err(), Some(None), "the old pick finds no model at all");
        // Switched on again: the old pick is its model again.
        helpers.set(vec![model_helper("own-b", "Сервер", "http://192.168.1.21:7780/home/v1/llm", false), a]);
        brain.keep_helper_models().unwrap();
        assert_eq!(row_of(&brain, "home-a").unwrap().id, "helper-home-a");
        assert_eq!(brain.pick_of("helper-home-a", "qwen2.5").err(), Some(Some("Кабинет не на связи".into())));
    }

    /// A list asked in the background that fails — the helper's model server not up yet, a key not taken — is asked
    /// again at the next occasion, so «the next helper» does not pass that helper by for the rest of the start.
    #[tokio::test]
    async fn a_list_that_could_not_be_had_is_asked_again_at_the_next_occasion() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![ModelHelper { key: Some("not-taken".into()), ..model_helper("home-b", "Б", &url, true) }]);
        brain.keep_helper_models().unwrap();
        let row = brain.snapshot().0.into_iter().find(|row| row.helper() == Some("home-b")).unwrap();
        let asked = (row.id.clone(), row.created_at.clone());
        let mut waited = 0;
        while brain.listed.lock().unwrap().contains(&asked) {
            assert!(waited < 500, "the failed request lets the row go");
            tokio::time::sleep(Duration::from_millis(10)).await;
            waited += 1;
        }
        assert!(row_of(&brain, "home-b").unwrap().models.is_empty());

        helpers.set(vec![model_helper("home-b", "Б", &url, true)]);
        brain.keep_helper_models().unwrap();
        let mut waited = 0;
        while row_of(&brain, "home-b").unwrap().models.is_empty() {
            assert!(waited < 500, "asked again, and listed");
            tokio::time::sleep(Duration::from_millis(10)).await;
            waited += 1;
        }
        assert_eq!(row_of(&brain, "home-b").unwrap().models, ["big-model", "small-model"]);
        assert!(brain.listed.lock().unwrap().contains(&asked), "listed: not asked again this start");
    }

    /// The default backup's start: the main model on a helper, the helper's row made.
    fn on_a_helper() -> (Arc<BrainService>, Arc<FakeHelpers>, Arc<SecretsService>) {
        let (brain, helpers, secrets) = with_helpers();
        helpers.set(vec![model_helper(
            "home-computer",
            "Компьютер дома",
            "http://192.168.1.20:7780/home/v1/llm",
            false,
        )]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        choose(&brain, json!({"fast": {"provider": row.id, "model": "home"}}));
        (brain, helpers, secrets)
    }

    #[tokio::test]
    async fn the_gateway_sets_a_fast_cloud_backup_when_the_main_model_is_a_helpers_and_says_so() {
        let (brain, _helpers, _) = on_a_helper();
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        assert_eq!(brain.view().note, None, "one can be taken: nothing to ask of the person");
        let mut events = brain.events.subscribe();
        brain.keep_backup();
        let view = brain.view();
        assert_eq!(
            view.choice.backup,
            Some(BrainPick { provider: claude.id.clone(), model: "claude-haiku-5-5".into() })
        );
        assert_eq!(view.choice.backup_by, Some(BackupBy::Gateway));
        let note = view.note.unwrap();
        assert_eq!(note.kind, "backupAuto");
        assert!(
            note.text.contains("claude-haiku-5-5 (Claude)") && note.text.contains("могут стоить денег"),
            "{}",
            note.text
        );
        assert_eq!(serde_json::to_value(&view.choice).unwrap()["backupBy"], "gateway");
        assert_eq!(changes(&mut events, "backup.auto"), 1);
        brain.keep_backup();
        assert_eq!(changes(&mut events, "backup.auto"), 0, "set once");
        // A gateway whose settings the computer keeps takes the computer's choice.
        let (brain, _helpers, _) = on_a_helper();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        brain.set_owner(Arc::new(|| false));
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup, None);
    }

    #[tokio::test]
    async fn a_main_model_elsewhere_in_the_home_gets_no_default_backup() {
        let (brain, _helpers, _) = with_helpers();
        let nas = brain
            .add(&input(json!({"preset": "custom-openai", "baseUrl": "http://192.168.1.50:11434/v1"})), None)
            .unwrap();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        choose(&brain, json!({"fast": {"provider": nas.id, "model": "llama"}}));
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.note), (None, None));
    }

    #[tokio::test]
    async fn a_hint_counts_only_by_its_exact_name() {
        let (brain, _helpers, _) = on_a_helper();
        let openai = brain.add(&input(json!({"preset": "openai", "key": "sk-openai-test-0001"})), None).unwrap();
        listed(&brain, &openai.id, &["codex-mini-latest", "gpt-5-mini-2026-01", "gpt-5.5-pro"]);
        brain.keep_backup();
        let view = brain.view();
        assert_eq!(view.choice.backup, None, "no part of a name is guessed");
        assert_eq!(view.note.unwrap().kind, "backupNeeded");
        listed(&brain, &openai.id, &["gpt-5-mini", "gpt-5-mini-2026-01"]);
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup.unwrap().model, "gpt-5-mini");

        let (brain, _helpers, _) = on_a_helper();
        let gemini = brain.add(&input(json!({"preset": "gemini", "key": "gemini-test-key-0001"})), None).unwrap();
        listed(&brain, &gemini.id, &["models/gemini-2.5-flash", "models/gemini-2.5-pro"]);
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup.unwrap().model, "models/gemini-2.5-flash", "the listed id is stored");
    }

    #[tokio::test]
    async fn openrouter_without_hints_asks_for_a_backup() {
        let (brain, _helpers, _) = on_a_helper();
        let router = brain.add(&input(json!({"preset": "openrouter", "key": "sk-or-test-0001"})), None).unwrap();
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup, None);
        listed(&brain, &router.id, &["anthropic/claude-haiku-5-5", "openai/gpt-5-mini"]);
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.note.unwrap().kind), (None, "backupNeeded"));
    }

    #[tokio::test]
    async fn a_backup_the_person_took_away_is_not_set_again() {
        let (brain, _helpers, _) = on_a_helper();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        brain.keep_backup();
        assert!(brain.view().choice.backup.is_some());
        choose(&brain, json!({"backup": null}));
        assert_eq!(brain.view().choice.backup_by, Some(BackupBy::Declined));
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.note), (None, None), "the person decided");
        // A backup the person picks is theirs.
        let claude = view.providers.iter().find(|row| row.preset == "claude").unwrap().id.clone();
        choose(&brain, json!({"backup": {"provider": claude, "model": "claude-sonnet-5-5"}}));
        assert_eq!(brain.view().choice.backup_by, None);
    }

    #[tokio::test]
    async fn let_the_gateway_choose_hands_the_backup_back_to_it() {
        let (brain, _helpers, _) = on_a_helper();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        // «не нужна», then «пусть выберет шлюз» (owner's decision 18): the refusal goes, the gateway chooses again.
        choose(&brain, json!({"backup": null}));
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.choice.backup_by), (None, Some(BackupBy::Declined)));
        choose(&brain, json!({"backup": "auto"}));
        assert_eq!(brain.view().choice.backup_by, None, "nobody declines any more");
        brain.keep_backup();
        let view = brain.view();
        assert_eq!(view.choice.backup_by, Some(BackupBy::Gateway));
        let taken = view.choice.backup.clone().expect("the gateway chose a backup");
        // «Пусть выберет шлюз» again: its own pick goes too, and it chooses anew — the same model while it qualifies.
        choose(&brain, json!({"backup": "auto"}));
        assert_eq!(brain.view().choice.backup, None);
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup.as_ref(), view.choice.backup_by), (Some(&taken), Some(BackupBy::Gateway)));
        // A person's backup goes back to the gateway too.
        let claude = view.providers.iter().find(|row| row.preset == "claude").unwrap().id.clone();
        choose(&brain, json!({"backup": {"provider": claude, "model": "claude-sonnet-5-5"}}));
        assert_eq!(brain.view().choice.backup_by, None);
        choose(&brain, json!({"backup": "auto"}));
        assert_eq!(brain.view().choice.backup, None);
        brain.keep_backup();
        assert_eq!(brain.view().choice.backup, Some(taken));
        // Only the backup is the gateway's to choose; the view says that this gateway takes "auto".
        assert!(brain.choose(&object(json!({"fast": "auto"})), None).is_err());
        assert_eq!(serde_json::to_value(brain.view().choice).unwrap()["autoBackup"], true);
    }

    #[tokio::test]
    async fn the_gateways_own_backup_goes_with_its_provider_and_a_persons_stays_theirs() {
        let (brain, _helpers, _) = on_a_helper();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        let openai = brain.add(&input(json!({"preset": "openai", "key": "sk-test-openai-0001"})), None).unwrap();
        listed(&brain, &openai.id, &["gpt-5-mini", "gpt-5"]);
        brain.keep_backup();
        let first = brain.view().choice.backup.expect("the gateway chose a backup");
        // Removing the provider of the gateway's own pick is not refused: the pick goes with it, and the gateway
        // chooses again among the clouds left.
        brain.remove(&first.provider, None).unwrap();
        assert_eq!(brain.view().choice.backup, None);
        brain.keep_backup();
        let view = brain.view();
        let next = view.choice.backup.expect("another cloud stands in");
        assert_ne!(next.provider, first.provider);
        assert_eq!(view.choice.backup_by, Some(BackupBy::Gateway));
        // A person's backup stays theirs: its provider is not removed until they change it.
        choose(&brain, json!({"backup": {"provider": next.provider, "model": next.model}}));
        let refused = brain.remove(&next.provider, None).unwrap_err();
        assert_eq!(refused.status, axum::http::StatusCode::CONFLICT);
        assert!(refused.message.contains("«пусть выберет центр»"), "{}", refused.message);
        assert!(brain.view().choice.backup.is_some());
    }

    #[tokio::test]
    async fn no_default_backup_while_the_regulator_keeps_clouds_shut() {
        let (brain, helpers, _) = on_a_helper();
        brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        helpers.shut(&[Reach::Foreign]);
        brain.keep_backup();
        let view = brain.view();
        assert_eq!(view.choice.backup, None);
        assert_eq!(view.note.unwrap().kind, "backupClosed");
    }

    #[tokio::test]
    async fn a_cloud_main_model_gets_no_default_backup() {
        let (brain, _helpers, _) = with_helpers();
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        brain.add(&input(json!({"preset": "openai", "key": "sk-openai-test-0001"})), None).unwrap();
        choose(&brain, json!({"fast": {"provider": claude.id, "model": "claude-sonnet-5-5"}}));
        brain.keep_backup();
        let view = brain.view();
        assert_eq!((view.choice.backup, view.note), (None, None));
    }

    #[tokio::test]
    async fn slot_of_a_helper_away_falls_back_to_the_profile() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper("home-computer", "Компьютер дома", &url, true)]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap();
        assert_eq!(BrainSource::slot_of(&*brain, &row.id, "small-model").unwrap().provider, row.id);
        helpers.up("home-computer", false);
        assert!(BrainSource::slot_of(&*brain, &row.id, "small-model").is_none());
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        assert!(BrainSource::slot_of(&*brain, &claude.id, "claude-haiku-5-5").is_some());
        helpers.shut(&[Reach::Foreign]);
        assert!(BrainSource::slot_of(&*brain, &claude.id, "claude-haiku-5-5").is_none(), "kept in by the regulator");
    }

    /// The stage-3 follow-up: a row a choice answers with stays when its helper is switched off, removed or kept out by
    /// the regulator — and is never asked then, while «Где сейчас» says nobody gives the model.
    #[tokio::test]
    async fn a_chosen_row_whose_helper_is_switched_off_removed_or_kept_out_is_never_asked() {
        let (at_helper, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        let own = ModelHelper {
            kind: HelperKind::Own,
            reach: Reach::Mine,
            ..model_helper("own-1", "Мой сервер", &url, true)
        };
        helpers.set(vec![own.clone()]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "own-1").unwrap();
        choose(&brain, json!({"fast": {"provider": row.id, "model": "small-model"}}));
        assert_eq!(said(&brain.slot(ModelProfile::Fast).unwrap()).await.as_deref(), Ok("Да."));
        assert_eq!(at_helper.chats.lock().unwrap().len(), 1);

        // Kept out by «Только дома».
        helpers.shut(&[Reach::Mine, Reach::Foreign]);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err("Модель в облаке, а облака сейчас закрыты.".into()));
        // Switched off, or removed: gone from the helpers, the row kept while it answers.
        helpers.shut(&[]);
        helpers.set(Vec::new());
        brain.keep_helper_models().unwrap();
        assert!(row_of(&brain, "own-1").is_some());
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(said(&slot).await, Err("Мой сервер не на связи, а запасной модели нет.".into()));
        assert_eq!(at_helper.chats.lock().unwrap().len(), 1, "never asked again");
        assert_eq!(brain.now(ModelProfile::Fast).state, Answering::Off);
    }

    /// Two model servers of this computer (stage 6) are two, each looked at by itself: the one a choice points at
    /// closed, the other stands in as the backup; both closed, one sentence says so.
    #[tokio::test]
    async fn two_model_servers_of_this_computer_stand_in_for_each_other() {
        let (brain, helpers, _) = with_helpers();
        let ollama =
            brain.add(&input(json!({"preset": "ollama", "baseUrl": "http://127.0.0.1:11434/v1"})), None).unwrap();
        let lmstudio =
            brain.add(&input(json!({"preset": "lmstudio", "baseUrl": "http://127.0.0.1:1234/v1"})), None).unwrap();
        let server = |row: &ProviderView, up: bool| ModelHelper {
            provider: Some(row.id.clone()),
            builtin: true,
            key: None,
            ..model_helper("self", "Этот компьютер", row.base_url.as_deref().unwrap(), up)
        };
        helpers.set(vec![server(&ollama, false), server(&lmstudio, true)]);
        let pick = |row: &ProviderView, model: &str| json!({"provider": row.id, "model": model});
        choose(&brain, json!({"fast": pick(&ollama, "llama"), "backup": pick(&lmstudio, "qwen")}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), (lmstudio.id.as_str(), "qwen"), "no wait, no words");
        assert_eq!(brain.now(ModelProfile::Fast).reason.as_deref(), Some(BUILTIN_AWAY));
        assert_eq!(brain.view().note, None, "a backup on another server of this computer is a backup");

        helpers.set(vec![server(&ollama, false), server(&lmstudio, false)]);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!(
            said(&slot).await,
            Err("Ни основная, ни запасная модель на этом компьютере сейчас не отвечают.".into())
        );
    }

    /// A model server of this computer on the line says its own failure, as before the helpers (a model not pulled
    /// there: Ollama's own words), without a backup: neither another server's model in its place nor the helpers'
    /// words. Closed — its look — another server of this computer answers at once; a backup on a closed one is said so.
    #[tokio::test]
    async fn a_model_server_of_this_computer_on_the_line_says_its_own_failure() {
        let (brain, helpers, _) = with_helpers();
        let ollama =
            brain.add(&input(json!({"preset": "ollama", "baseUrl": "http://127.0.0.1:11434/v1"})), None).unwrap();
        let lmstudio =
            brain.add(&input(json!({"preset": "lmstudio", "baseUrl": "http://127.0.0.1:1234/v1"})), None).unwrap();
        listed(&brain, &lmstudio.id, &["qwen2.5-7b"]);
        let server = |row: &ProviderView, up: bool| ModelHelper {
            provider: Some(row.id.clone()),
            builtin: true,
            key: None,
            ..model_helper("self", "Этот компьютер", row.base_url.as_deref().unwrap(), up)
        };
        helpers.set(vec![server(&ollama, true), server(&lmstudio, true)]);
        let pick = |row: &ProviderView, model: &str| json!({"provider": row.id, "model": model});
        choose(&brain, json!({"fast": pick(&ollama, "llama3.1")}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.backup.is_none()), (ollama.id.as_str(), true), "nothing behind it");

        helpers.set(vec![server(&ollama, false), server(&lmstudio, true)]);
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), (lmstudio.id.as_str(), "qwen2.5-7b"));

        helpers.set(vec![server(&ollama, true), server(&lmstudio, false)]);
        choose(&brain, json!({"fast": pick(&ollama, "llama3.1"), "backup": pick(&lmstudio, "qwen2.5-7b")}));
        let slot = brain.slot(ModelProfile::Fast).unwrap();
        let behind = slot.backup.as_ref().unwrap().slot.clone();
        assert_eq!(said(&behind).await, Err("Модель не ответила, а запасная модель сейчас не отвечает.".into()));
    }

    /// A helper on the line whose model is not ready — its model server closed there (its contract says so, stage 6) —
    /// is not away: «Где сейчас», a pick of its model and the words say its model is silent, not the helper offline.
    #[tokio::test]
    async fn a_helper_on_the_line_whose_model_is_silent_is_not_said_to_be_offline() {
        let (_service, url) = service().await;
        let (brain, helpers, _) = with_helpers();
        let silent = ModelHelper {
            state: HelperState::Up,
            ..model_helper("home-computer", "Компьютер дома", "http://192.168.1.20:7780/home/v1/llm", false)
        };
        helpers.set(vec![silent.clone()]);
        brain.keep_helper_models().unwrap();
        let row = row_of(&brain, "home-computer").unwrap().id;
        let main = json!({"provider": row, "model": "home"});
        choose(&brain, json!({ "fast": main }));
        let w1 = "Компьютер дома: модель сейчас не отвечает, а запасной модели нет.";
        assert_eq!(said(&brain.slot(ModelProfile::Fast).unwrap()).await, Err(w1.into()));
        assert_eq!(brain.now(ModelProfile::Fast).reason.as_deref(), Some(w1));
        let why = "Компьютер дома: модель сейчас не отвечает";
        assert_eq!(brain.pick_of(&row, "home").err(), Some(Some(why.into())));

        // A backup at home answers in its place, and «Где сейчас» says why.
        let local = brain.add(&input(json!({"preset": "custom-openai", "baseUrl": url})), None).unwrap();
        choose(&brain, json!({"fast": main, "backup": {"provider": local.id, "model": "small-model"}}));
        assert_eq!(brain.slot(ModelProfile::Fast).unwrap().provider, local.id);
        assert_eq!(brain.now(ModelProfile::Fast).reason.as_deref(), Some(why));

        // W2: the cloud backup kept in.
        let claude = brain.add(&input(json!({"preset": "claude", "key": "sk-ant-test-0001"})), None).unwrap();
        choose(&brain, json!({"fast": main, "backup": {"provider": claude.id, "model": "claude-haiku-5-5"}}));
        helpers.shut(&[Reach::Mine, Reach::Foreign]);
        assert_eq!(
            said(&brain.slot(ModelProfile::Fast).unwrap()).await,
            Err("Компьютер дома: модель сейчас не отвечает, а облака сейчас закрыты — запасная модель не ответит."
                .into())
        );
        helpers.shut(&[]);

        // W3: the backup's helper away besides.
        helpers.set(vec![silent, model_helper("home-b", "Мастерская", "http://192.168.1.21:7780/home/v1/llm", false)]);
        brain.keep_helper_models().unwrap();
        let away = row_of(&brain, "home-b").unwrap().id;
        choose(&brain, json!({"fast": main, "backup": {"provider": away, "model": "big-model"}}));
        assert_eq!(
            said(&brain.slot(ModelProfile::Fast).unwrap()).await,
            Err("Ни основная, ни запасная модель сейчас не отвечают.".into())
        );
    }

    #[tokio::test]
    async fn a_helpers_row_is_the_gateways_own_never_offered_added_or_edited() {
        let (brain, helpers, _) = with_helpers();
        helpers.set(vec![model_helper(
            "home-computer",
            "Компьютер дома",
            "http://192.168.1.20:7780/home/v1/llm",
            false,
        )]);
        brain.keep_helper_models().unwrap();
        let view = brain.view();
        assert!(view.presets.iter().all(|preset| preset.id != "helper"), "never offered");
        let row = row_of(&brain, "home-computer").unwrap();
        assert_eq!(
            serde_json::to_value(&row.helper).unwrap(),
            json!({"id": "home-computer", "name": "Компьютер дома", "kind": "home", "state": "down"})
        );
        assert!(!row.cloud && row.local, "at home");
        let added = brain.add(&input(json!({"preset": "helper"})), None).unwrap_err();
        assert_eq!((added.status.as_u16(), added.message.as_str()), (400, "Модель Турбо появляется здесь сама."));
        let renamed = brain.update(&row.id, &input(json!({"title": "Моя"})), None).unwrap_err();
        assert_eq!(renamed.status.as_u16(), 409);
        assert!(renamed.message.contains("«Дом»"), "{}", renamed.message);
        assert_eq!(brain.remove(&row.id, None).unwrap_err().status.as_u16(), 409);
        // The person's own rows stay theirs.
        let own = brain.add(&input(json!({"preset": "ollama"})), None).unwrap();
        assert!(own.helper.is_none());
        brain.update(&own.id, &input(json!({"title": "Моя"})), None).unwrap();
        brain.remove(&own.id, None).unwrap();
    }
}
