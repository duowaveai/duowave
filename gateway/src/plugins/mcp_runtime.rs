//! MCP servers as plugins (`"runtime": "mcp"`, docs/plans/plugins.md, stage 8 and «MCP в обе стороны»): a package
//! that names an MCP server on the network (`mcp.url`) and the tools of it the plugin gives — each tool of its manifest
//! is a tool of that server (`handler` — the server's name for it). The admin sees in the manifest what reaches
//! «Мозг»; nothing else of the server does. The address must be on a host of the plugin's `net:` rights; an
//! `apiKey` setting, when the manifest has one, goes as the bearer token.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::host_api::checked_url;
use super::runtime::{
    CallKind, HostApi, MemoryStats, PluginCode, PluginError, PluginInstance, PluginRuntime, LOAD_LIMIT,
};
use crate::mcp::mcp_connection::{open_connection, ConnectionAuth, ConnectionSpec, McpConnection};
use crate::mcp::mcp_presets::TransportKind;

pub struct McpRuntime;

struct Instance {
    connection: Mutex<Option<Arc<McpConnection>>>,
    tools: Vec<String>,
}

/// A tool's result as the plugin's answer: its structured content, else its text.
fn answer(result: crate::mcp::mcp_tools::ToolCallResult) -> Result<Value, PluginError> {
    if result.is_error {
        let text: Vec<String> = result
            .content
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        return Err(PluginError::Script(format!("сервер MCP: {}", text.join(" "))));
    }
    if let Some(structured) = result.structured_content {
        return Ok(structured);
    }
    let text: Vec<&str> = result.content.iter().filter_map(|item| item.get("text").and_then(Value::as_str)).collect();
    Ok(Value::String(text.join("\n")))
}

impl PluginInstance for Instance {
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>> {
        let connection = self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let function = function.to_string();
        Box::pin(async move {
            let connection = connection.ok_or_else(|| PluginError::Unavailable("плагин выгружен".into()))?;
            let arguments: Map<String, Value> = match input {
                Value::Object(map) => map,
                Value::Null => Map::new(),
                other => return Err(PluginError::Script(format!("вход инструмента — объект, а не {other}"))),
            };
            let limit = kind.limit();
            match connection.call(&function, arguments, &CancellationToken::new(), limit).await {
                Ok(result) => answer(result),
                Err(crate::mcp::mcp_connection::CallError::Timeout) => {
                    Err(PluginError::Timeout(limit.as_millis() as u64))
                }
                Err(error) => Err(PluginError::Script(format!("сервер MCP: {error}"))),
            }
        })
    }

    fn memory(&self) -> BoxFuture<'static, Option<MemoryStats>> {
        // The server is elsewhere: nothing of it is in the gateway's memory but the connection.
        Box::pin(async { None })
    }

    fn exports(&self) -> Vec<String> {
        self.tools.clone()
    }

    fn stop(&self) {
        let connection = self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let (Some(connection), Ok(runtime)) = (connection, tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move { connection.close().await });
        }
    }
}

impl PluginRuntime for McpRuntime {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn start(
        &self,
        code: PluginCode,
        host: Arc<dyn HostApi>,
    ) -> BoxFuture<'static, Result<Arc<dyn PluginInstance>, PluginError>> {
        Box::pin(async move {
            let url = code.endpoint.clone().ok_or_else(|| PluginError::Load("нет mcp.url".into()))?;
            let url = checked_url(&code.hosts, &url).map_err(PluginError::Load)?;
            let token =
                host.settings().get("apiKey").and_then(Value::as_str).map(str::to_string).filter(|key| !key.is_empty());
            let spec = ConnectionSpec {
                id: format!("plugin-{}", code.id),
                transport: TransportKind::Http,
                command: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                url: Some(url.to_string()),
                headers: Vec::new(),
                auth: match token {
                    Some(token) => ConnectionAuth::Token(Arc::new(move || Some(token.clone()))),
                    None => ConnectionAuth::None,
                },
            };
            let log_host = host.clone();
            let opened = tokio::time::timeout(
                LOAD_LIMIT + Duration::from_secs(8),
                open_connection(&spec, Arc::new(move |line: String| log_host.log("info", &line))),
            )
            .await
            .map_err(|_| PluginError::Load("сервер MCP не ответил вовремя".into()))?
            .map_err(|error| PluginError::Load(format!("сервер MCP не подключился: {error}")))?;
            let connection = Arc::new(opened.0);
            let tools = connection
                .tools()
                .await
                .map_err(|error| PluginError::Load(format!("сервер MCP не дал список инструментов: {error}")))?
                .into_iter()
                .map(|tool| tool.name)
                .collect();
            Ok(Arc::new(Instance { connection: Mutex::new(Some(connection)), tools }) as Arc<dyn PluginInstance>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A host with nothing but its settings: the engine reads only the key.
    struct Host;

    impl HostApi for Host {
        fn fetch(&self, _: Value) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn kv_get(&self, _: &str) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn kv_set(&self, _: &str, _: Value) -> Result<(), String> {
            Ok(())
        }
        fn kv_delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn kv_keys(&self, _: &str) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn say(&self, _: String, _: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
            Box::pin(async { Ok(false) })
        }
        fn settings(&self) -> Map<String, Value> {
            Map::new()
        }
        fn log(&self, _: &str, _: &str) {}
    }

    fn code(endpoint: Option<&str>, hosts: &[&str]) -> PluginCode {
        PluginCode {
            id: "weather-mcp".into(),
            version: "1.0.0".into(),
            endpoint: endpoint.map(str::to_string),
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
            memory_mb: 4,
            ..PluginCode::default()
        }
    }

    async fn refused(code: PluginCode) -> String {
        match McpRuntime.start(code, Arc::new(Host)).await {
            Err(PluginError::Load(why)) => why,
            Err(other) => panic!("not a load error: {other}"),
            Ok(_) => panic!("started"),
        }
    }

    #[test]
    fn gives_the_servers_structured_answer_else_its_text_and_its_error_as_one() {
        let structured = crate::mcp::mcp_tools::ToolCallResult::from_json(
            &json!({ "content": [{ "type": "text", "text": "+5" }], "structuredContent": { "celsius": 5 } }),
        );
        assert_eq!(answer(structured).unwrap(), json!({ "celsius": 5 }));
        let text = crate::mcp::mcp_tools::ToolCallResult::from_json(
            &json!({ "content": [{ "type": "text", "text": "+5," }, { "type": "text", "text": "облачно" }] }),
        );
        assert_eq!(answer(text).unwrap(), json!("+5,\nоблачно"));
        let failed = crate::mcp::mcp_tools::ToolCallResult::from_json(
            &json!({ "content": [{ "type": "text", "text": "нет города" }], "isError": true }),
        );
        assert!(matches!(answer(failed), Err(PluginError::Script(why)) if why.contains("нет города")));
    }

    #[tokio::test]
    async fn reaches_only_a_server_on_a_host_of_its_rights_and_never_this_computer() {
        assert!(refused(code(None, &["weather.example"])).await.contains("нет mcp.url"));
        let elsewhere = refused(code(Some("https://other.example/mcp"), &["weather.example"])).await;
        assert!(elsewhere.contains("net:other.example"), "{elsewhere}");
        let here = refused(code(Some("http://127.0.0.1:9/mcp"), &["127.0.0.1"])).await;
        assert!(here.contains("адрес этого компьютера"), "{here}");
    }
}
