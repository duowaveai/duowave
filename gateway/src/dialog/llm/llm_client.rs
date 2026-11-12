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
