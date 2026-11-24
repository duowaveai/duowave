//! The words about a model the floors below the brain use too: the tokens of one turn, which the request journal sums
//! (core/requests.rs). The model's clients and the conversation are the brain's (src/dialog/llm).

use serde::{Deserialize, Serialize};

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
