//! The MCP servers for the work systems' plugs (docs/plans/work-systems.md, «Plugs», 3): a card's verb `via: "mcp"`
//! calls a tool of a server the person added in «Мозг» → «MCP», named by its id or its title. T3 Code stops this way.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::mcp_service::McpService;
use crate::talk::system::McpCaller;

pub struct SystemTools(pub Arc<McpService>);

#[async_trait]
impl McpCaller for SystemTools {
    fn on_line(&self, server: &str) -> bool {
        self.0.on_line(server)
    }

    async fn call(&self, server: &str, tool: &str, args: Map<String, Value>) -> Result<Value, String> {
        self.0.call_for_system(server, tool, args).await
    }
}
