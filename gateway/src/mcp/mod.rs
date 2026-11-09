//! MCP servers: their connections and tools. The station's sessions take tools from here (`McpService::offer` and
//! `McpOffer::run`); the panel's test chat («Спросить мозг») asks the model with them.

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::Router;

use crate::app::App;

pub mod ask_controller;
pub mod mcp_connection;
pub mod mcp_controller;
pub mod mcp_import;
mod mcp_legacy_sse;
pub mod mcp_oauth;
pub mod mcp_presets;
pub mod mcp_prompt;
pub mod mcp_service;
pub mod mcp_tools;

#[cfg(test)]
mod fixtures;

pub use ask_controller::{AskDialog, AskEvent, AskModel, AskToolbox};
pub use mcp_service::{Cancelled, McpOffer, McpService, OfferOptions, OfferServer, ToolOutcome};

/// The largest body these routes read: Express's default for JSON, as the Node gateway had it. A body is read before
/// the route asks who is calling, so its size is the bound on what anyone who reaches the port can make us parse.
pub const BODY_LIMIT: usize = 100 * 1024;

/// `/api/v1/mcp/…` and the check chat, `/api/v1/brain/ask`.
pub fn routes() -> Router<Arc<App>> {
    mcp_controller::routes().merge(ask_controller::routes()).layer(DefaultBodyLimit::max(BODY_LIMIT))
}
