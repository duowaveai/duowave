//! The memory's tools for the model (docs/plans/memory.md, «Голос»): `remember`, `forget` and `recall`, given the
//! way a connector gives its tools, so they reach the station's answers and the panel's check chat with the speaker
//! known. `recall` changes nothing and stays offered after a text from outside the house; `remember` and `forget` do
//! not: a page from the internet must not write the household's memory. Off, the memory offers nothing; paused, it
//! offers no `remember`.

use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use super::model::{EntityKind, Source};
use super::service::{origin_of, MemoryError, MemoryService, Remember, Remembered};
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContext, ToolContribution, ToolRun,
};
use crate::core::identity::permissions::use_of;
use crate::dialog::llm::LlmTool;
use crate::skills::capabilities::CapabilityName;

/// How many records `recall` returns.
const RECALL_LIMIT: usize = 7;

pub struct MemoryConnector {
    memory: Weak<MemoryService>,
}

impl MemoryConnector {
    pub fn new(memory: &Arc<MemoryService>) -> MemoryConnector {
        MemoryConnector { memory: Arc::downgrade(memory) }
    }
}

impl Connector for MemoryConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "memory".into(),
            // Not listed: the memory has its own section in the panel; the kind is only a label.
            kind: ConnectorKind::Lookup,
            title: crate::tr!("Память"),
            description: crate::tr!("Что персонажи помнят о доме: запомнить, забыть, вспомнить"),
            cloud: false,
            env: Vec::new(),
            capabilities: Some(vec![CapabilityName::Memory]),
            setup: None,
        }
    }

    fn listed(&self) -> bool {
        false
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move {
            match self.memory.upgrade() {
                Some(memory) if memory.enabled() => ConnectorStatus::ready(crate::tr!("включена")),
                Some(_) => ConnectorStatus::off(crate::tr!("выключена")),
                None => ConnectorStatus::off(crate::tr!("не запущена")),
            }
        })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        let Some(memory) = self.memory.upgrade() else { return Vec::new() };
        if !memory.enabled() {
            return Vec::new();
        }
        let mut tools = Vec::new();
        if memory.writing().is_ok() {
            tools.push(contribution(remember_tool(), false, run_remember(&memory)));
        }
        tools.push(contribution(forget_tool(), false, run_forget(&memory)));
        tools.push(contribution(recall_tool(), true, run_recall(&memory)));
        tools
    }
}

fn contribution(tool: LlmTool, read_only: bool, run: ToolRun) -> ToolContribution {
    ToolContribution { tool, permission: use_of(CapabilityName::Memory), run, read_only, outside: false }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

pub fn remember_tool() -> LlmTool {
    LlmTool {
        name: "remember".into(),
        description: "Keep a fact about the home and its people for later conversations: who is who, where, with \
                      whom, habits, dates, likes. Write the fact as one short sentence in the language of the \
                      conversation."
            .into(),
        parameters: object(
            json!({
                "fact": { "type": "string", "description": "The fact as one short sentence: «У Маши аллергия на орехи»." },
                "about": { "type": "string", "description": "Who or what it is about, by name (Маша, кот Барсик, кухня); leave out for the home itself." },
                "about_kind": { "type": "string", "enum": ["person", "pet", "room", "thing", "place", "org"], "description": "What `about` is." },
                "relation": { "type": "string", "description": "A short label of the relation or property: «сестра», «работает в», «аллергия»." },
                "other": { "type": "string", "description": "Another person or thing the fact links to: Петя in «Маша — сестра Пети»." },
                "other_kind": { "type": "string", "enum": ["person", "pet", "room", "thing", "place", "org"] },
                "replaces": { "type": "string", "description": "An old fact this one replaces, as you know it («плавание по средам»): it is closed with today's date." }
            }),
            &["fact"],
        ),
    }
}

pub fn forget_tool() -> LlmTool {
    LlmTool {
        name: "forget".into(),
        description: "Close a remembered fact the person asks to forget or says is no longer true; it stays in the \
                      history, no longer used."
            .into(),
        parameters: object(
            json!({ "what": { "type": "string", "description": "The fact, in a few words: «орехи», «Петя плавание»." } }),
            &["what"],
        ),
    }
}

pub fn recall_tool() -> LlmTool {
    LlmTool {
        name: "recall".into(),
        description: "Look up what is remembered about a person, a pet, a place or a topic of this home, beyond the \
                      lines you already have."
            .into(),
        parameters: object(
            json!({ "query": { "type": "string", "description": "A name or a few words: «Маша», «аллергия»." } }),
            &["query"],
        ),
    }
}

fn text_of(input: &Map<String, Value>, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

fn kind_of(input: &Map<String, Value>, key: &str) -> Option<EntityKind> {
    input.get(key).and_then(Value::as_str).and_then(EntityKind::parse).filter(|kind| *kind != EntityKind::Household)
}

fn run_remember(memory: &Arc<MemoryService>) -> ToolRun {
    let memory = Arc::downgrade(memory);
    Arc::new(move |input: Map<String, Value>, context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let memory = memory.clone();
        Box::pin(async move {
            let memory = memory.upgrade().ok_or_else(|| anyhow::anyhow!("memory is not running"))?;
            let character = memory.answering().id;
            let station = context.station.as_ref().map(|station| station.device.as_str());
            let origin = origin_of(&context.principal, station, &character);
            let wanted = Remember {
                fact: text_of(&input, "fact").unwrap_or_default(),
                about: text_of(&input, "about"),
                about_kind: kind_of(&input, "about_kind"),
                relation: text_of(&input, "relation"),
                other: text_of(&input, "other"),
                other_kind: kind_of(&input, "other_kind"),
                replaces: text_of(&input, "replaces"),
                pinned: false,
            };
            Ok(match memory.remember(wanted, origin, Source::Model, Some(&context.principal)) {
                Ok(Remembered::Added { fact, closed: Some(old) }) => {
                    format!("Remembered: {}. Closed the old fact: {}.", fact.text, old.text)
                }
                Ok(Remembered::Added { fact, closed: None }) => format!("Remembered: {}.", fact.text),
                Ok(Remembered::Known(fact)) => format!("Already known: {}.", fact.text),
                Err(error @ (MemoryError::Paused | MemoryError::Off | MemoryError::Full)) => {
                    format!("Not remembered: {error}")
                }
                Err(error) => anyhow::bail!("{error}"),
            })
        })
    })
}

fn run_forget(memory: &Arc<MemoryService>) -> ToolRun {
    let memory = Arc::downgrade(memory);
    Arc::new(move |input: Map<String, Value>, context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let memory = memory.clone();
        Box::pin(async move {
            let memory = memory.upgrade().ok_or_else(|| anyhow::anyhow!("memory is not running"))?;
            let what = text_of(&input, "what").ok_or_else(|| anyhow::anyhow!("what is empty"))?;
            let viewer = memory.viewer_for(&context.principal);
            Ok(match memory.forget(&what, &viewer, Some(&context.principal))? {
                Some(fact) => format!("Closed: {}.", fact.text),
                None => "Nothing like that is remembered.".to_string(),
            })
        })
    })
}

fn run_recall(memory: &Arc<MemoryService>) -> ToolRun {
    let memory = Arc::downgrade(memory);
    Arc::new(move |input: Map<String, Value>, context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let memory = memory.clone();
        Box::pin(async move {
            let memory = memory.upgrade().ok_or_else(|| anyhow::anyhow!("memory is not running"))?;
            let query = text_of(&input, "query").ok_or_else(|| anyhow::anyhow!("query is empty"))?;
            let viewer = memory.viewer_for(&context.principal);
            let found = memory.recall(&query, &viewer, RECALL_LIMIT)?;
            if found.is_empty() {
                return Ok("Nothing remembered about that.".to_string());
            }
            Ok(found.iter().map(|view| format!("- {}", view.fact.text)).collect::<Vec<_>>().join("\n"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::roles::Role;
    use crate::dialog::PersonaService;
    use crate::settings::SettingsService;
    use crate::storage::Db;
    use tokio_util::sync::CancellationToken;

    fn memory() -> Arc<MemoryService> {
        let config = Config::for_tests(&Env::default()).dialog;
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let personas = Arc::new(PersonaService::new(&config, db.clone(), settings.clone(), events.clone()).unwrap());
        Arc::new(MemoryService::new(db, settings, events, personas))
    }

    fn context() -> ToolContext {
        ToolContext {
            principal: household_principal(Role::Member, None),
            cancel: CancellationToken::new(),
            station: None,
        }
    }

    fn call(tools: &[ToolContribution], name: &str, input: Value) -> BoxFuture<'static, anyhow::Result<String>> {
        let tool = tools.iter().find(|t| t.tool.name == name).expect("offered");
        (tool.run)(input.as_object().unwrap().clone(), context())
    }

    #[tokio::test]
    async fn remembers_recalls_and_forgets_through_the_tools() {
        let memory = memory();
        let connector = MemoryConnector::new(&memory);
        assert!(!connector.listed());
        let tools = connector.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.tool.name.as_str()).collect();
        assert_eq!(names, vec!["remember", "forget", "recall"]);
        assert!(tools.iter().all(|t| t.permission == "use:memory"));
        assert!(tools.iter().find(|t| t.tool.name == "recall").unwrap().read_only);
        assert!(!tools.iter().find(|t| t.tool.name == "remember").unwrap().read_only);

        let said = call(
            &tools,
            "remember",
            json!({"fact": "Маша — сестра Пети", "about": "Маша", "relation": "сестра", "other": "Петя"}),
        )
        .await
        .unwrap();
        assert_eq!(said, "Remembered: Маша — сестра Пети.");
        let found = call(&tools, "recall", json!({"query": "Петя"})).await.unwrap();
        assert!(found.contains("Маша — сестра Пети"), "linked facts are found from either side: {found}");
        assert_eq!(
            call(&tools, "forget", json!({"what": "сестра Пети"})).await.unwrap(),
            "Closed: Маша — сестра Пети."
        );
        assert_eq!(call(&tools, "recall", json!({"query": "Маша"})).await.unwrap(), "Nothing remembered about that.");
    }

    #[tokio::test]
    async fn offers_less_when_paused_and_nothing_when_off() {
        let memory = memory();
        let connector = MemoryConnector::new(&memory);
        memory.pause(crate::memory::commands::PauseSpan::UntilResumed, None, None).unwrap();
        let names: Vec<String> = connector.tools().into_iter().map(|t| t.tool.name).collect();
        assert_eq!(names, vec!["forget", "recall"]);
        memory.set_enabled(false, None).unwrap();
        assert!(connector.tools().is_empty());
    }
}
