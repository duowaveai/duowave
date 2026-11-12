//! Model providers from the panel and the choice of models (src/dialog/llm/brain.rs): the brain the dialog asks per
//! answer. Rows and the choice are read at construction (the dialog says which model it uses at start) and after
//! every change; clients are built per provider when first needed and rebuilt when it or the key changes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use futures::StreamExt;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::presets::{headers_for, model_for, preset_of, suggested_of, ProviderKind, ProviderPreset, PROVIDER_PRESETS};
use crate::config::{DialogConfig, DialogProvider, ModelPair, ModelProfile, LLM_EFFORTS};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::dialog::llm::brain::{
    serialize_kind, Backup, BrainModels, BrainOrigin, BrainProfile, BrainProfiles, BrainSource, BrainStatus, EnvBrain,
    ModelSlot, ENV_PROVIDER,
};
use crate::dialog::llm::openai_chat::{tokens_field_for, OPENAI_BASE_URL};
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
use crate::settings::definitions::{Brain, BrainPick, BRAIN};
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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceView {
    pub fast: Option<BrainPick>,
    pub smart: Option<BrainPick>,
    pub effort: String,
    /// Answers when the chosen model gives no answer; `env` as the provider: the .env model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<BrainPick>,
}

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
        r"(?i)ECONNREFUSED|ENOTFOUND|EAI_AGAIN|fetch failed|cannot reach|ECONNRESET|connection refused|dns error|error sending request",
    )
    .unwrap()
});
static TIMED_OUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)timeout|timed out|aborted|TimeoutError").unwrap());

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
        };
        service.reload()?;
        Ok(service)
    }

    // The panel.

    pub fn view(&self) -> BrainView {
        let (rows, choice) = self.snapshot();
        let mut presets: Vec<&'static ProviderPreset> = PROVIDER_PRESETS.iter().collect();
        presets.sort_by_key(|preset| preset.order);
        let env_status = self.env.status();
        BrainView {
            presets,
            providers: rows.iter().map(|row| self.provider_view(row)).collect(),
            choice: ChoiceView {
                fast: choice.fast.clone(),
                smart: choice.smart.clone(),
                effort: self.effort_of(&choice),
                backup: choice.backup.clone(),
            },
            status: self.status_of(&rows, &choice),
            env: EnvView {
                provider: self.config.provider,
                title: env_status
                    .profiles
                    .fast
                    .as_ref()
                    .map(|profile| profile.title.clone())
                    .unwrap_or_else(|| "нет".into()),
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
            .ok_or_else(|| ApiError::bad_request("Неизвестный провайдер."))?;
        let base_url = address(preset, input.get("baseUrl"))?;
        let title = Some(text(input.get("title"), 60)).filter(|title| !title.is_empty()).unwrap_or(preset.title.into());
        let options = fields(preset, input.get("options"));
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
            let view = self.provider_view(&self.row(&id)?);
            (id, view)
        };
        self.events.emit(GatewayEvent::BrainChanged { change: "provider.added".into(), provider_id: Some(id), actor });
        Ok(view)
    }

    /// Changes what the input names, nothing else; the key only of a provider that is still there.
    pub fn update(&self, id: &str, input: &ProviderInput, actor: Option<Actor>) -> Result<ProviderView, ApiError> {
        let view = {
            let _mutation = self.lock();
            let row = self.stored_row(id)?.ok_or_else(missing)?;
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
            self.provider_view(&self.row(id)?)
        };
        self.events.emit(GatewayEvent::BrainChanged {
            change: "provider.updated".into(),
            provider_id: Some(id.to_string()),
            actor,
        });
        Ok(view)
    }

    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        {
            let _mutation = self.lock();
            if !self.ids()?.iter().any(|known| known == id) {
                return Err(missing());
            }
            let choice = self.settings.peek(&BRAIN)?.unwrap_or_default();
            let answering = |pick: &Option<BrainPick>| pick.as_ref().is_some_and(|pick| pick.provider == id);
            if answering(&choice.fast) || answering(&choice.smart) {
                return Err(ApiError::conflict("Этот провайдер сейчас отвечает. Сначала выберите другую модель."));
            }
            if answering(&choice.backup) {
                return Err(ApiError::conflict("Этот провайдер — запасная модель. Сначала выберите другую запасную."));
            }
            let flow = self.stored_row(id)?.and_then(|row| token_flow(&row.preset));
            if self.db.with(|c| c.execute("DELETE FROM llm_providers WHERE id = ?1", params![id]))? == 0 {
                return Err(missing());
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
            .ok_or_else(|| ApiError::bad_request("Этот провайдер подключается только ключом."))?;
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

    /// The models: fast and smart (null: back to .env; a smart left out follows the fast one), the backup (null: none;
    /// `env` as the provider: the .env model) and Claude's depth.
    pub fn choose(&self, input: &Map<String, Value>, actor: Option<Actor>) -> Result<BrainView, ApiError> {
        let mutation = self.lock();
        let rows = self.load_rows()?;
        let mut next = self.settings.peek(&BRAIN)?.unwrap_or_default();
        for profile in ["fast", "smart", "backup"] {
            let Some(value) = input.get(profile) else { continue };
            let pick = if value.is_null() {
                None
            } else {
                let provider = value.get("provider").and_then(Value::as_str);
                // The .env model may be the backup when it can answer.
                let env = profile == "backup" && self.env.slot(ModelProfile::Fast).is_some();
                let known =
                    |provider: &&str| rows.iter().any(|row| row.id == *provider) || (env && *provider == ENV_PROVIDER);
                let Some(provider) = provider.filter(known) else {
                    return Err(ApiError::bad_request("Такого провайдера нет."));
                };
                let model = text(value.get("model"), 200);
                if model.is_empty() {
                    return Err(ApiError::bad_request("Выберите модель."));
                }
                Some(BrainPick { provider: provider.to_string(), model })
            };
            match profile {
                "fast" => next.fast = pick,
                "smart" => next.smart = pick,
                _ => next.backup = pick,
            }
        }
        if let Some(effort) = input.get("effort") {
            match effort.as_str().filter(|effort| LLM_EFFORTS.contains(effort)) {
                Some(effort) => next.effort = Some(effort.to_string()),
                None => return Err(ApiError::bad_request("Глубина мышления: low, medium, high, xhigh или max.")),
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
        let models = self.list_models(&row, &settings).await?;
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
                list_problem.unwrap_or_else(|| "Сервис не назвал ни одной модели: впишите её вручную.".into());
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

    /// A provider's row and its settings with its key, read together under `mutation`: they belong to one provider.
    fn paired(&self, id: &str) -> Result<(ProviderRow, LlmSettings), ApiError> {
        let _mutation = self.lock();
        let row = self.stored_row(id)?.ok_or_else(missing)?;
        let settings = self.settings_of(&row);
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

    fn provider_view(&self, row: &ProviderRow) -> ProviderView {
        let preset = preset_of(&row.preset);
        let cloud = cloud_of_row(row);
        ProviderView {
            id: row.id.clone(),
            preset: row.preset.clone(),
            kind: row.kind,
            title: row.title.clone(),
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

    fn settings_of(&self, row: &ProviderRow) -> LlmSettings {
        let preset = preset_of(&row.preset);
        let base_url = row.base_url.clone().or_else(|| preset.and_then(|preset| preset.base_url).map(str::to_string));
        LlmSettings {
            provider: row.kind.provider(),
            model: String::new(),
            models: ModelPair { fast: String::new(), smart: String::new() },
            tokens_field: tokens_field_for(base_url.as_deref().unwrap_or(OPENAI_BASE_URL)),
            base_url,
            api_key: self.secrets.get(&key_of(&row.id)),
            max_tokens: self.config.max_tokens,
            effort: self.effort(),
            explicit: true,
            headers: headers_for(&row.preset, &row.options),
            access: self.access_of(row).map(|access| Access(access)),
            api: preset.map(|preset| preset.api).unwrap_or_default(),
        }
    }

    /// A provider's model as one answer's slot, without a backup. The caller holds `mutation`.
    fn slot_of_row(&self, row: &ProviderRow, model: &str) -> ModelSlot {
        ModelSlot {
            client: self.client_of(row),
            model: model_for(&row.preset, &row.options, model),
            provider: row.id.clone(),
            kind: row.kind.provider(),
            title: row.title.clone(),
            cloud: cloud_of_row(row),
            backup: None,
        }
    }

    /// The provider's client, rebuilt when the row (or the provider under its id), its key or the thinking depth
    /// changed. The caller holds `mutation`: the row and the key it reads belong to one provider.
    fn client_of(&self, row: &ProviderRow) -> Arc<dyn LlmClient> {
        let stamp = format!(
            "{}|{}|{}|{}",
            row.created_at,
            row.updated_at,
            self.secrets.mask(&key_of(&row.id)).hint.unwrap_or_default(),
            self.effort()
        );
        let mut clients = self.clients.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(built) = clients.get(&row.id).filter(|built| built.stamp == stamp) {
            return built.client.clone();
        }
        let client = build(&self.settings_of(row));
        clients.insert(row.id.clone(), Built { stamp, client: client.clone() });
        client
    }

    fn status_of(&self, rows: &[ProviderRow], choice: &Brain) -> BrainStatus {
        let Some(fast) = Self::picked(rows, choice, ModelProfile::Fast) else { return self.env.status() };
        let smart = Self::picked(rows, choice, ModelProfile::Smart).unwrap_or(fast);
        let profile = |(row, pick): (&ProviderRow, &BrainPick)| BrainProfile {
            provider: row.id.clone(),
            kind: row.kind.provider(),
            title: row.title.clone(),
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
            return "Нет ключа.".into();
        }
        if KEY_REFUSED.is_match(&message) {
            return "Ключ не принят.".into();
        }
        if NOT_FOUND.is_match(&message) {
            return "Модель или адрес не найдены.".into();
        }
        if REFUSED.is_match(&message) {
            return "Сервис отказал: лимит или пустой счёт.".into();
        }
        if UNREACHABLE.is_match(&message) {
            return "Адрес не отвечает.".into();
        }
        if TIMED_OUT.is_match(&message) {
            return format!("Не ответил за {} секунд.", CHECK_TIMEOUT.as_secs());
        }
        format!("Ошибка: {}", head(&message, 200))
    }
}

fn missing() -> ApiError {
    ApiError::not_found("Такого провайдера нет.")
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

/// A row's questions leave the house: Claude always, others unless their address is at home.
fn cloud_of_row(row: &ProviderRow) -> bool {
    if row.kind == ProviderKind::Anthropic {
        return true;
    }
    row.base_url.as_deref().is_none_or(|url| !is_local_url(Some(url)))
}

/// The address: the preset's own, or the given one when the preset lets it change.
fn address(preset: &ProviderPreset, value: Option<&Value>) -> Result<Option<String>, ApiError> {
    if !preset.editable_url {
        return Ok(preset.base_url.map(str::to_string));
    }
    let given = text(value, 300);
    let url = if given.is_empty() { preset.base_url.unwrap_or("").to_string() } else { given };
    if url.is_empty() {
        return Err(ApiError::bad_request("Укажите адрес сервиса."));
    }
    match url::Url::parse(&url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => Ok(Some(url.trim_end_matches('/').to_string())),
        _ => Err(ApiError::bad_request("Адрес должен начинаться с http:// или https://.")),
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

impl BrainSource for BrainService {
    /// The profile's model, and the backup with it when one is chosen and it is another model.
    fn slot(&self, profile: ModelProfile) -> Option<ModelSlot> {
        // The row and the key its client is built with, from between the panel's changes, never across one.
        let _mutation = self.lock();
        let (rows, choice) = self.snapshot();
        let mut slot = match Self::picked(&rows, &choice, profile) {
            Some((row, pick)) => self.slot_of_row(row, &pick.model),
            None => self.env.slot(profile)?,
        };
        let backup = choice.backup.as_ref().and_then(|pick| match rows.iter().find(|row| row.id == pick.provider) {
            Some(row) => Some(self.slot_of_row(row, &pick.model)),
            None if pick.provider == ENV_PROVIDER => self.env.slot(profile),
            None => None,
        });
        slot.backup = backup
            .filter(|backup| {
                (backup.provider.as_str(), backup.model.as_str()) != (slot.provider.as_str(), slot.model.as_str())
            })
            .map(|backup| Arc::new(Backup::new(backup)));
        Some(slot)
    }

    fn slot_of(&self, provider: &str, model: &str) -> Option<ModelSlot> {
        let _mutation = self.lock();
        let (rows, _) = self.snapshot();
        let row = rows.iter().find(|row| row.id == provider)?;
        Some(self.slot_of_row(row, model))
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
}
