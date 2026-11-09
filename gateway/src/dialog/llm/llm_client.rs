//! The model's side of a dialog, whatever the provider: a request (instructions, turns, tools) and a stream of what
//! the model writes. Claude (anthropic.rs), any OpenAI-compatible /chat/completions (openai_chat.rs) and the echo
//! for checks implement `LlmClient`.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::anthropic::AnthropicLlm;
use super::echo::EchoLlm;
use super::openai_chat::{tokens_field_for, OpenAiChatLlm, OPENAI_BASE_URL};
use crate::config::{DialogConfig, DialogProvider, ModelPair};

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
        }
    }
}

/// The client for these settings; None for `none`, the dialog is off.
pub fn create_llm(settings: &LlmSettings) -> Option<Arc<dyn LlmClient>> {
    match settings.provider {
        DialogProvider::Anthropic => Some(Arc::new(AnthropicLlm::new(settings.clone()))),
        DialogProvider::OpenAi => Some(Arc::new(OpenAiChatLlm::new(settings.clone()))),
        DialogProvider::Echo => Some(Arc::new(EchoLlm)),
        DialogProvider::None => None,
    }
}
