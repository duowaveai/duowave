//! The model's side of a dialog, whatever the provider: a request (instructions, turns, tools) and a stream of what
//! the model writes. Claude (anthropic.rs), any OpenAI-compatible /chat/completions (openai_chat.rs), OpenAI's
//! /responses for the ChatGPT plan (openai_responses.rs) and the echo for checks implement `LlmClient`.
//!
//! A client takes its bearer right before each request (`bearer_of`): a key, or the access a sign-in keeps fresh
//! (src/oauth) — tokens that live an hour are renewed there, ahead of time and once for everyone waiting.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::anthropic::AnthropicLlm;
use super::echo::EchoLlm;
use super::openai_chat::{tokens_field_for, OpenAiChatLlm, OPENAI_BASE_URL};
use super::openai_responses::OpenAiResponsesLlm;
use crate::config::{DialogConfig, DialogProvider, ModelPair};

/// Which of OpenAI's APIs an OpenAI-kind provider speaks: /chat/completions (every compatible service) or /responses
/// (the ChatGPT plan, which takes nothing else).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmApi {
    #[default]
    Chat,
    Responses,
}

/// How a provider's refusals are said (`ProviderPreset.errors`): as they come, or DuoWave's own codes in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorWords {
    #[default]
    Plain,
    DuoWave,
}

/// What DuoWave's last refusal asks of the owner: connect again (`revoked`, `model` — the model is not in the plan,
/// the profiles come back to the service's defaults), the account (`ended`, `quota`), or wait (`busy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessProblem {
    Revoked,
    Ended,
    Quota,
    Busy,
    Model,
}

/// A provider's last refusal of DuoWave's kind, shared by its clients and its card: set by a refusal, cleared by an
/// answer or a new sign-in. Memory only: a restart starts clean, the next request tells again.
#[derive(Debug, Clone, Default)]
pub struct ProblemCell(Arc<std::sync::Mutex<Option<AccessProblem>>>);

impl ProblemCell {
    pub fn get(&self) -> Option<AccessProblem> {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set(&self, problem: Option<AccessProblem>) {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = problem;
    }
}

/// «Через минуту» or «через N мин.»: how long `Retry-After` asks to wait, rounded up to minutes.
fn wait_words(after: Option<std::time::Duration>) -> String {
    match after.map(|after| after.as_secs().div_ceil(60)) {
        None | Some(0) => crate::tr!("позже"),
        Some(1) => crate::tr!("через минуту"),
        Some(minutes) => crate::tr!("через {minutes} мин", minutes = minutes),
    }
}

/// A DuoWave refusal in words (docs/plans/duowave-connect.md, step 5): our code is in `error.code` (Chat Completions,
/// Responses); Messages has Anthropic's `error.type` only, so there the status decides. The cell learns what the
/// owner should do. None — not one of ours: the plain error stays.
pub fn duowave_refusal(
    status: u16,
    body: &str,
    after: Option<std::time::Duration>,
    cell: &ProblemCell,
) -> Option<anyhow::Error> {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let code = parsed.pointer("/error/code").and_then(serde_json::Value::as_str).unwrap_or_default().to_lowercase();
    let (problem, phrase) = match (status, code.as_str()) {
        (401, _) => (AccessProblem::Revoked, crate::tr!("Подключение DuoWave отключено. Подключите DuoWave заново.")),
        (402, "quota_exhausted") => (AccessProblem::Quota, crate::tr!("Лимит DuoWave на эту неделю исчерпан.")),
        (402, _) => (AccessProblem::Ended, crate::tr!("Доступ к DuoWave закончился. Загляните в кабинет DuoWave.")),
        (429, _) => (
            AccessProblem::Busy,
            crate::tr!("Слишком много запросов к DuoWave. Попробуйте {when}.", when = wait_words(after)),
        ),
        (403, "model_not_in_plan") => {
            (AccessProblem::Model, crate::tr!("Этой модели нет в вашем тарифе DuoWave. Подключите DuoWave заново."))
        }
        _ => return None,
    };
    cell.set(Some(problem));
    let code = if code.is_empty() { String::new() } else { format!(": {code}") };
    Some(anyhow::Error::new(SpokenFailure {
        phrase: phrase.to_string(),
        detail: format!("DuoWave: HTTP {status}{code}"),
    }))
}

/// `Retry-After` in seconds (the form our services send; a date is not read).
pub fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}

/// Where a client takes its bearer before each request when a sign-in keeps it (src/oauth).
pub trait AccessSource: Send + Sync {
    /// The bearer for the next request: renewed first when it is about to run out.
    fn bearer(&self) -> BoxFuture<'_, anyhow::Result<String>>;
    /// A request went through: a limit noted earlier is over.
    fn ok(&self) {}
    /// The service said the plan's limit is reached: the panel shows it on the provider.
    fn limited(&self) {}
}

/// The access in a client's settings. No Debug of what it holds.
#[derive(Clone)]
pub struct Access(pub Arc<dyn AccessSource>);

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Access(…)")
    }
}

/// A failure the station says in its own words instead of the error phrase, because the owner can do something about
/// it (the ChatGPT plan's limit, a sign-in to renew). `detail` is for the log and the panel.
#[derive(Debug)]
pub struct SpokenFailure {
    pub phrase: String,
    pub detail: String,
}

impl fmt::Display for SpokenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for SpokenFailure {}

/// «Выход в интернет» (src/egress): the provider goes through the person's server and that server does not answer —
/// the call is refused at once, in words the owner can act on, rather than wait out a timeout. Nothing goes out
/// directly: a key from a country the service refuses would risk the account.
pub fn egress_refusal(url: &str) -> Option<anyhow::Error> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_string();
    crate::egress::refused_host(&host).then(|| {
        anyhow::Error::new(SpokenFailure {
            phrase: crate::tr!("VPN не отвечает: модель сейчас недоступна. Проверьте подключение в приложении «VPN».")
                .to_string(),
            detail: format!("{host}: the VPN connection does not answer («Выход в интернет»)"),
        })
    })
}

/// The bearer of the next request: the access's when a sign-in gives it, else the key; None without either.
pub async fn bearer_of(settings: &LlmSettings) -> anyhow::Result<Option<String>> {
    if let Some(access) = &settings.access {
        return access.0.bearer().await.map(Some);
    }
    Ok(settings.api_key.clone().filter(|key| !key.is_empty()))
}

/// A function the model may call: a station command, a lookup on our side or an MCP server's tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the input object.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Map<String, Value>,
    /// Why the call cannot run as written (arguments that are not a JSON object): said back to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

/// Tokens of one model turn, as the provider counts them: the cache shows here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub input: u64,
    pub output: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
}

/// The model's turn, as the conversation goes on after tool results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantTurn {
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The provider's own parts of the turn (Claude's thinking blocks, another's reasoning), sent back unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
    /// The turn stopped at its output limit before it called anything: its words or its thinking were cut short.
    /// Rounds that know what to do about it (the workshop agent's, `ToolBox::cut_answer_note`) go on and tell the
    /// model; anywhere else the turn ends the answer as it is, a spoken one too.
    #[serde(skip)]
    pub out_of_tokens: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmMessage {
    User(String),
    Assistant(AssistantTurn),
    Tool(Vec<ToolResult>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    /// The tools stay in the request (the history holds calls) but the model must answer in words.
    None,
}

#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    /// The part of the instructions that stays the same between questions: a provider may cache it.
    pub system: String,
    /// What changes with every question (the time, a lookup's data): after the cached part.
    pub system_tail: Option<String>,
    /// Earlier turns, then the question (and after a tool call, the model's turn and the results).
    pub messages: Vec<LlmMessage>,
    /// The profile's model; the provider's default model without it.
    pub model: Option<String>,
    pub tools: Vec<LlmTool>,
    pub tool_choice: ToolChoice,
    /// An output limit of its own instead of the client's (LLM_MAX_TOKENS), for this request only: the workshop's
    /// agent writes files (WORKSHOP_MAX_TOKENS), a spoken answer is short (SPEECH_MAX_TOKENS). The ChatGPT plan sends
    /// no limit at all.
    pub max_tokens: Option<u32>,
    /// Cache the history as well as the stable instructions: Claude marks the last message too, so the next round of
    /// a long run reads all before it from the cache. The other clients do not read it.
    pub cache_history: bool,
}

/// A turn cut off by its output limit in the middle of a tool call. The call is not run: its input is cut short.
/// Rounds that know what to do about it (the workshop agent's, `ToolBox::cut_note`) go on and tell the model;
/// anywhere else the answer fails with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallCut {
    /// Who ran out, in the log's words: «Claude», «the model».
    pub who: &'static str,
    /// The tool it was calling.
    pub tool: String,
    /// The file the call was writing, when its input named it before the cut.
    pub path: Option<String>,
}

impl ToolCallCut {
    /// A cut call of `tool`; its file is read from the input written so far.
    pub fn new(who: &'static str, tool: &str, partial_input: &str) -> ToolCallCut {
        ToolCallCut { who, tool: tool.to_string(), path: path_in(partial_input) }
    }
}

impl fmt::Display for ToolCallCut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ran out of tokens in a tool call", self.who)
    }
}

impl std::error::Error for ToolCallCut {}

/// An answer the service did not finish through no fault of the request: the connection could not be made, broke or
/// ended early mid-answer, or the service was overloaded. The same request may go through a little later. Rounds that
/// may ask again (the workshop agent's, `ToolBox::retries`) do; anywhere else the answer fails with it. A turn's calls
/// run only once the turn is whole, so none of an interrupted turn's ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted(pub String);

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Interrupted {}

/// `Interrupted` with its message, as an `anyhow` error.
pub fn interrupted(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Interrupted(message.into()))
}

/// The `path` of a tool input cut short: the JSON string after `"path":`, when it was written whole.
fn path_in(partial: &str) -> Option<String> {
    let (_, after) = partial.split_once("\"path\"")?;
    let mut chars = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?.chars();
    let mut path = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(path).filter(|path| !path.is_empty()),
            '\\' => path.push(chars.next()?),
            c => path.push(c),
        }
    }
    None
}

/// Text as the model writes it; once the turn is over, its tool calls and the turn itself.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmEvent {
    Text(String),
    Tool(ToolCall),
    Turn(AssistantTurn),
}

pub type LlmStream = BoxStream<'static, anyhow::Result<LlmEvent>>;

pub trait LlmClient: Send + Sync {
    /// Model and provider, for logs and health.
    fn description(&self) -> String;
    /// Whether the model can be offered tools.
    fn tools(&self) -> bool;
    /// What the model writes; `cancel` stops it (the station cancelled, a newer question came).
    fn stream(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream;
}

/// The name of the output limit: OpenAI's own API wants max_completion_tokens, the compatible ones max_tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokensField {
    #[default]
    MaxTokens,
    MaxCompletionTokens,
}

/// What a client is built from: the model part of the dialog settings. From .env (DIALOG_PROVIDER, LLM_*) or from a
/// provider the panel added; `explicit` ones never fall back to the environment's keys and addresses.
#[derive(Debug, Clone)]
pub struct LlmSettings {
    pub provider: DialogProvider,
    pub model: String,
    pub models: ModelPair,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub max_tokens: u32,
    pub effort: String,
    /// Use only the key and address given here: a panel provider must not borrow ANTHROPIC_API_KEY.
    pub explicit: bool,
    /// Extra request headers a provider needs (YandexGPT's folder).
    pub headers: BTreeMap<String, String>,
    pub tokens_field: TokensField,
    /// The bearer from a sign-in, taken fresh before each request, in place of `api_key`.
    pub access: Option<Access>,
    /// /chat/completions or /responses, for an OpenAI-kind provider.
    pub api: LlmApi,
    /// DuoWave's: its refusals said in words (`duowave_refusal`), the last one kept here for the card.
    pub duowave: Option<ProblemCell>,
}

/// "model" or, when the profiles differ, "fast model-a, smart model-b".
pub fn describe_models(models: &ModelPair) -> String {
    if models.fast == models.smart {
        models.fast.clone()
    } else {
        format!("fast {}, smart {}", models.fast, models.smart)
    }
}

impl LlmSettings {
    /// The model .env describes (DIALOG_PROVIDER, LLM_*). Claude also takes what its SDK read from the environment
    /// itself: ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) without LLM_API_KEY, ANTHROPIC_BASE_URL without LLM_BASE_URL.
    pub fn from_dialog(config: &DialogConfig) -> LlmSettings {
        let claude = config.provider == DialogProvider::Anthropic;
        let base_url = if claude {
            config.base_url.clone().or_else(|| config.anthropic_base_url.clone())
        } else {
            config.base_url.clone()
        };
        let api_key = if claude {
            config.api_key.clone().or_else(|| config.anthropic_key.clone())
        } else {
            config.api_key.clone()
        };
        LlmSettings {
            provider: config.provider,
            model: config.model.clone(),
            models: config.models.clone(),
            tokens_field: tokens_field_for(base_url.as_deref().unwrap_or(OPENAI_BASE_URL)),
            base_url,
            api_key,
            max_tokens: config.max_tokens,
            effort: config.effort.clone(),
            explicit: false,
            headers: BTreeMap::new(),
            access: None,
            api: LlmApi::Chat,
            duowave: None,
        }
    }
}

/// The client for these settings; None for `none`, the dialog is off.
pub fn create_llm(settings: &LlmSettings) -> Option<Arc<dyn LlmClient>> {
    match settings.provider {
        DialogProvider::Anthropic => Some(Arc::new(AnthropicLlm::new(settings.clone()))),
        DialogProvider::OpenAi if settings.api == LlmApi::Responses => {
            Some(Arc::new(OpenAiResponsesLlm::new(settings.clone())))
        }
        DialogProvider::OpenAi => Some(Arc::new(OpenAiChatLlm::new(settings.clone()))),
        DialogProvider::Echo => Some(Arc::new(EchoLlm)),
        DialogProvider::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_call_names_its_file_when_the_input_wrote_it_whole() {
        let cut = ToolCallCut::new("Claude", "write_file", r#"{"path": "panel.js", "content": "export function"#);
        assert_eq!(cut.path.as_deref(), Some("panel.js"));
        assert_eq!(cut.to_string(), "Claude ran out of tokens in a tool call");
        assert_eq!(
            ToolCallCut::new("the model", "write_file", r#"{"path":"ui/main.js","content":"#).path.as_deref(),
            Some("ui/main.js")
        );
        assert_eq!(ToolCallCut::new("the model", "write_file", r#"{"path": "pan"#).path, None, "cut inside the name");
        assert_eq!(ToolCallCut::new("the model", "write_file", r#"{"content": "x"#).path, None);
        assert_eq!(ToolCallCut::new("the model", "write_file", "").path, None);
    }
}
