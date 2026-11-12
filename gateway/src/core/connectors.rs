//! A connector is anything the gateway talks to beyond the station: a model (LLM, recognition, voice), the smart home,
//! a messenger, calls, a lookup service, a catalog — and, next, MCP servers and agents (OpenClaw, Hermes). Each one
//! declares itself with a manifest (what it is, whether data leaves the house, what it gives) and reports its state;
//! one that gives the model tools contributes them with the right each tool needs. The registry is how the panel
//! lists them and how a new connector joins without changes to the core (ROADMAP: «участник сообщества добавляет
//! своё подключение, не меняя ядро шлюза»).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::identity::{can, Principal};
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::ToolOutcome;
use crate::skills::capabilities::CapabilityName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    /// The voice link to Yandex.
    Station,
    Llm,
    Stt,
    Tts,
    SmartHome,
    Messenger,
    Calls,
    Lookup,
    Catalog,
    Mcp,
    Agent,
    /// Plugins: skills, tools and sources from packages (src/plugins).
    Plugins,
}

/// Every kind with its title for people, in the panel's order.
pub const CONNECTOR_KINDS: &[(ConnectorKind, &str)] = &[
    (ConnectorKind::Station, "Связь с Яндексом"),
    (ConnectorKind::Llm, "Модель для разговора"),
    (ConnectorKind::Stt, "Распознавание речи"),
    (ConnectorKind::Tts, "Голос"),
    (ConnectorKind::SmartHome, "Умный дом"),
    (ConnectorKind::Messenger, "Мессенджеры"),
    (ConnectorKind::Calls, "Звонки"),
    (ConnectorKind::Lookup, "Справки: погода, новости"),
    (ConnectorKind::Catalog, "Каталоги моделей"),
    (ConnectorKind::Mcp, "MCP-серверы: инструменты для модели"),
    (ConnectorKind::Agent, "Помощники-агенты"),
    (ConnectorKind::Plugins, "Приложения: навыки, инструменты и источники"),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorManifest {
    /// Stable id: `telegram`, `home-assistant`, `mcp:filesystem`.
    pub id: String,
    pub kind: ConnectorKind,
    pub title: String,
    pub description: String,
    /// Data leaves the house through it (DESIGN.md: whatever goes outside is amber).
    pub cloud: bool,
    /// The env variables it still reads; the ones set up in the panel read none.
    pub env: Vec<String>,
    /// The capabilities it serves when the routes send them its way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<CapabilityName>>,
    /// Where people set it up; none: nowhere (a catalog, an MCP server has its own sheet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<ConnectorSetup>,
}

/// Where a connector is set up in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorSetup {
    /// Its own sheet in «Дом» → «Коннекторы» (src/connectors/settings.rs).
    Panel,
    /// «Мозг» → «Модели».
    Brain,
    /// «Голос».
    Voice,
    /// The station's mode on the dashboard.
    Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorState {
    /// Configured and answering.
    Ready,
    /// Configured; its state is not checked (or not checkable).
    Configured,
    /// Not set up: what it needs is missing. The gateway works without it.
    Unset,
    /// Switched off (its settings kept), or not part of this build.
    Off,
    /// Not in this build, but on the way: people are told it is coming, not asked to configure it.
    Soon,
    /// Configured and failing now.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectorStatus {
    pub state: ConnectorState,
    /// For people: what it uses or why it fails, never a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ConnectorStatus {
    pub fn new(state: ConnectorState, detail: Option<String>) -> ConnectorStatus {
        ConnectorStatus { state, detail }
    }

    pub fn ready(detail: impl Into<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Ready, Some(detail.into()))
    }

    pub fn configured(detail: Option<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Configured, detail)
    }

    pub fn unset(detail: impl Into<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Unset, Some(detail.into()))
    }

    pub fn off(detail: impl Into<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Off, Some(detail.into()))
    }

    pub fn soon(detail: impl Into<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Soon, Some(detail.into()))
    }

    pub fn error(detail: impl Into<String>) -> ConnectorStatus {
        ConnectorStatus::new(ConnectorState::Error, Some(detail.into()))
    }
}

/// What a tool runs with: who asked, the signal to stop, and — inside a station's answer — where its commands for
/// the station go.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub principal: Principal,
    pub cancel: CancellationToken,
    pub station: Option<StationAct>,
}

/// The station a tool's call answers for: a tool that acts (plays, speaks — docs/plans/plugins.md, «Инструмент
/// плагина не только находит, но и делает») adds its commands to that answer. None in the panel's check chat.
#[derive(Clone)]
pub struct StationAct {
    pub device: String,
    /// Where the station fetches our /media.
    pub media_base_url: Option<String>,
    /// Adds commands to the answer: false when the answer went out already.
    pub add: Arc<dyn Fn(Vec<crate::gateway::protocol::protocol_types::Directive>) -> bool + Send + Sync>,
}

impl std::fmt::Debug for StationAct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StationAct").field("device", &self.device).finish_non_exhaustive()
    }
}

pub type ToolRun =
    Arc<dyn Fn(Map<String, Value>, ToolContext) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>;

/// A tool a connector gives the model, with the right its use needs (checked per principal).
#[derive(Clone)]
pub struct ToolContribution {
    pub tool: LlmTool,
    pub permission: String,
    pub run: ToolRun,
    /// It changes nothing: it stays offered after a text from outside the house.
    pub read_only: bool,
    /// Its answer comes from outside the house (a service on the internet): what follows it is tainted.
    pub outside: bool,
}

/// The tools the connectors give, as the dialog asks for them per speaker (the station's conversations, the
/// panel's check chat).
pub trait ContributedTools: Send + Sync {
    fn tools(&self, principal: &Principal) -> Vec<ToolContribution>;
}

impl ContributedTools for ConnectorRegistry {
    fn tools(&self, principal: &Principal) -> Vec<ToolContribution> {
        ConnectorRegistry::tools(self, principal)
    }
}

/// The connectors' tools offered in one answer: what the model sees, and how to run a call of one.
pub struct ContributedOffer {
    pub tools: Vec<LlmTool>,
    index: HashMap<String, ToolContribution>,
}

impl ContributedOffer {
    /// None when there is nothing to offer. A name offered already (the station's tools, an MCP server's) keeps
    /// its owner: the contribution of that name is left out.
    pub fn new(contributions: Vec<ToolContribution>, taken: &[LlmTool]) -> Option<ContributedOffer> {
        let mut index = HashMap::new();
        let mut tools = Vec::new();
        for contribution in contributions {
            let name = contribution.tool.name.clone();
            if taken.iter().any(|tool| tool.name == name) || index.contains_key(&name) {
                continue;
            }
            tools.push(contribution.tool.clone());
            index.insert(name, contribution);
        }
        (!tools.is_empty()).then_some(ContributedOffer { tools, index })
    }

    pub fn owns(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// Whether a tool changes nothing (still offered after a text from outside the house).
    pub fn safe(&self, name: &str) -> bool {
        self.index.get(name).is_some_and(|contribution| contribution.read_only)
    }

    /// Runs the model's call; a failure is an answer the model reads.
    pub async fn run(&self, call: &ToolCall, principal: &Principal, cancel: CancellationToken) -> ToolOutcome {
        self.run_for(call, principal, cancel, None).await
    }

    /// The same inside a station's answer: a tool that acts adds its commands to it.
    pub async fn run_for(
        &self,
        call: &ToolCall,
        principal: &Principal,
        cancel: CancellationToken,
        station: Option<StationAct>,
    ) -> ToolOutcome {
        let Some(contribution) = self.index.get(&call.name) else {
            return ToolOutcome::error(format!("error: {} is not available now", call.name));
        };
        if let Some(error) = &call.error {
            return ToolOutcome::error(format!("error: {error}"));
        }
        let context = ToolContext { principal: principal.clone(), cancel, station };
        match (contribution.run)(call.input.clone(), context).await {
            Ok(text) if contribution.outside => ToolOutcome::tainted(text),
            Ok(text) => ToolOutcome::from(text),
            Err(error) => ToolOutcome::error(format!("error: {error}")),
        }
    }
}

pub trait Connector: Send + Sync {
    /// What it is; read on every look (the panel may move the model between home and the cloud).
    fn manifest(&self) -> ConnectorManifest;
    /// How it is doing now. A failure is a status (`error` with its reason), not a panic.
    fn status(&self) -> BoxFuture<'_, ConnectorStatus>;
    /// Tools for the model: MCP servers and agents give them; most connectors give none.
    fn tools(&self) -> Vec<ToolContribution> {
        Vec::new()
    }
    /// Whether the panel's list of connectors shows it. A built-in source of tools with a section of its own (the
    /// memory) is not a connection to set up.
    fn listed(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectorView {
    pub manifest: ConnectorManifest,
    pub status: ConnectorStatus,
}

/// A status check that hangs is an error, not a hung panel.
const STATUS_TIMEOUT: Duration = Duration::from_millis(2000);

/// The connectors the gateway knows, by id. Built-in ones register at start (src/connectors); a plug-in would
/// register the same way. Ids are unique: a second registration of an id is a mistake, not an override.
#[derive(Default)]
pub struct ConnectorRegistry {
    connectors: RwLock<Vec<Arc<dyn Connector>>>,
}

impl ConnectorRegistry {
    pub fn new() -> ConnectorRegistry {
        ConnectorRegistry::default()
    }

    pub fn register(&self, connector: Arc<dyn Connector>) -> anyhow::Result<()> {
        let id = connector.manifest().id;
        let mut connectors = self.connectors.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        if connectors.iter().any(|known| known.manifest().id == id) {
            anyhow::bail!("connector {id} is registered twice");
        }
        connectors.push(connector);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Connector>> {
        self.list().into_iter().find(|connector| connector.manifest().id == id)
    }

    /// In the order they registered.
    pub fn list(&self) -> Vec<Arc<dyn Connector>> {
        self.connectors.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Every connector with its state, checked at once; a slow check reads as an error.
    pub async fn views(&self) -> Vec<ConnectorView> {
        let connectors: Vec<Arc<dyn Connector>> =
            self.list().into_iter().filter(|connector| connector.listed()).collect();
        let checks = connectors.iter().map(|connector| async move {
            ConnectorView { manifest: connector.manifest(), status: checked(connector.as_ref()).await }
        });
        futures::future::join_all(checks).await
    }

    /// The tools the connectors give, of those the principal may use.
    pub fn tools(&self, principal: &Principal) -> Vec<ToolContribution> {
        self.list()
            .iter()
            .flat_map(|connector| connector.tools())
            .filter(|contribution| can(Some(principal), &contribution.permission))
            .collect()
    }
}

async fn checked(connector: &dyn Connector) -> ConnectorStatus {
    match tokio::time::timeout(STATUS_TIMEOUT, connector.status()).await {
        Ok(status) => status,
        Err(_) => ConnectorStatus::error("не ответил вовремя"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::{api_key_principal, system};
    use futures::FutureExt;

    struct Fake {
        id: &'static str,
        delay: Duration,
        tool_permission: Option<&'static str>,
    }

    impl Connector for Fake {
        fn manifest(&self) -> ConnectorManifest {
            ConnectorManifest {
                id: self.id.into(),
                kind: ConnectorKind::Mcp,
                title: self.id.into(),
                description: String::new(),
                cloud: false,
                env: Vec::new(),
                capabilities: None,
                setup: None,
            }
        }

        fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
            let delay = self.delay;
            async move {
                tokio::time::sleep(delay).await;
                ConnectorStatus::configured(None)
            }
            .boxed()
        }

        fn tools(&self) -> Vec<ToolContribution> {
            self.tool_permission
                .map(|permission| ToolContribution {
                    tool: LlmTool {
                        name: format!("{}_tool", self.id),
                        description: String::new(),
                        parameters: Value::Null,
                    },
                    permission: permission.into(),
                    run: Arc::new(|_, _| async { Ok(String::new()) }.boxed()),
                    read_only: false,
                    outside: false,
                })
                .into_iter()
                .collect()
        }
    }

    fn fake(id: &'static str) -> Arc<Fake> {
        Arc::new(Fake { id, delay: Duration::ZERO, tool_permission: None })
    }

    #[test]
    fn refuses_a_second_connector_with_the_same_id() {
        let registry = ConnectorRegistry::new();
        registry.register(fake("telegram")).unwrap();
        assert!(registry.register(fake("telegram")).is_err());
        assert!(registry.get("telegram").is_some());
        assert_eq!(registry.list().len(), 1);
    }

    #[tokio::test]
    async fn reads_a_check_that_hangs_as_an_error() {
        let registry = ConnectorRegistry::new();
        registry.register(fake("quick")).unwrap();
        registry
            .register(Arc::new(Fake { id: "slow", delay: Duration::from_secs(10), tool_permission: None }))
            .unwrap();
        let views = registry.views().await;
        assert_eq!(views[0].status, ConnectorStatus::configured(None));
        assert_eq!(views[1].status, ConnectorStatus::error("не ответил вовремя"));
    }

    #[test]
    fn gives_each_principal_the_tools_it_may_use() {
        let registry = ConnectorRegistry::new();
        registry
            .register(Arc::new(Fake { id: "home", delay: Duration::ZERO, tool_permission: Some("use:smart_home") }))
            .unwrap();
        registry
            .register(Arc::new(Fake { id: "files", delay: Duration::ZERO, tool_permission: Some("mcp.use") }))
            .unwrap();
        let key = api_key_principal("k", "script", &["mcp.use".into()]);
        let names = |principal: &Principal| -> Vec<String> {
            registry.tools(principal).into_iter().map(|contribution| contribution.tool.name).collect()
        };
        assert_eq!(names(&key), ["files_tool"]);
        assert_eq!(names(&system()), ["home_tool", "files_tool"]);
    }

    #[test]
    fn names_kinds_as_the_panel_reads_them() {
        assert_eq!(serde_json::to_value(ConnectorKind::SmartHome).unwrap(), "smart_home");
        assert_eq!(serde_json::to_value(ConnectorState::Configured).unwrap(), "configured");
        assert_eq!(CONNECTOR_KINDS.len(), 12);
    }

    fn contribution(
        name: &str,
        read_only: bool,
        outside: bool,
        answer: Result<&'static str, &'static str>,
    ) -> ToolContribution {
        ToolContribution {
            tool: LlmTool { name: name.into(), description: String::new(), parameters: Value::Null },
            permission: "use:plugins".into(),
            run: Arc::new(move |_, _| {
                async move { answer.map(str::to_string).map_err(|error| anyhow::anyhow!(error)) }.boxed()
            }),
            read_only,
            outside,
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), input: Map::new(), error: None }
    }

    #[tokio::test]
    async fn offers_the_contributed_tools_whose_names_are_free_and_marks_what_came_from_outside() {
        let taken = vec![LlmTool { name: "set_timer".into(), description: String::new(), parameters: Value::Null }];
        assert!(ContributedOffer::new(Vec::new(), &taken).is_none());
        let offer = ContributedOffer::new(
            vec![
                contribution("set_timer", true, false, Ok("чужой")),
                contribution("radio_search", true, true, Ok("[\"Джаз\"]")),
                contribution("radio_search", false, false, Ok("второй")),
                contribution("broken", false, false, Err("упал")),
            ],
            &taken,
        )
        .unwrap();
        let names: Vec<&str> = offer.tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["radio_search", "broken"]);
        assert!(!offer.owns("set_timer"), "a station's tool keeps its owner");
        assert!(offer.safe("radio_search") && !offer.safe("broken"));
        let found = offer.run(&call("radio_search"), &system(), CancellationToken::new()).await;
        assert_eq!(found, ToolOutcome::tainted("[\"Джаз\"]"));
        let failed = offer.run(&call("broken"), &system(), CancellationToken::new()).await;
        assert_eq!(failed, ToolOutcome::error("error: упал"));
        let missing = offer.run(&call("nothing"), &system(), CancellationToken::new()).await;
        assert!(missing.is_error);
    }
}
