//! The screen's tools for the brain (docs/plans/shared-folders.md, «Экран»), given the way the memory gives its own
//! (src/memory/tools.rs): which windows are open, «оставь», «закрой», pause and resume. What to show an app opens with
//! its own tools — «Папки»: `folder_show`, `folder_play`. Offered only where the DuoWave app opens the windows.
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use super::Screen;
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContext, ToolContribution, ToolRun,
};
use crate::core::identity::permissions::use_of;
use crate::dialog::llm::LlmTool;
use crate::skills::capabilities::CapabilityName;

pub struct ScreenConnector {
    screen: Weak<Screen>,
}

impl ScreenConnector {
    pub fn new(screen: &Arc<Screen>) -> ScreenConnector {
        ScreenConnector { screen: Arc::downgrade(screen) }
    }
}

const WINDOW: &str =
    "A window: its id from screen_windows, a part of its title, a kind (фото, видео, звук), «temporary» \
                      for every temporary window or «all».";

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn tools() -> Vec<(LlmTool, &'static str, bool)> {
    let window = || json!({ "window": { "type": "string", "description": WINDOW } });
    vec![
        (
            LlmTool {
                name: "screen_windows".into(),
                description: "The windows open now on the computer's screen, each with its id, title, kind (image, \
                              video, audio), whether it is kept (stays until closed) and paused. A temporary window \
                              closes when new windows open or when its video or sound ends."
                    .into(),
                parameters: object(json!({}), &[]),
            },
            "windows",
            true,
        ),
        (
            LlmTool {
                name: "screen_keep".into(),
                description: "Keep a window on the screen when the person says «оставь», «не закрывай»: it stays when \
                              new windows open and when its video ends, until it is closed. keep=false makes it \
                              temporary again."
                    .into(),
                parameters: object(
                    {
                        let mut properties = window();
                        properties["keep"] = json!({ "type": "boolean", "description": "false — temporary again" });
                        properties
                    },
                    &["window"],
                ),
            },
            "keep",
            false,
        ),
        (
            LlmTool {
                name: "screen_close".into(),
                description: "Close windows on the computer's screen, kept ones too.".into(),
                parameters: object(window(), &["window"]),
            },
            "close",
            false,
        ),
        (
            LlmTool {
                name: "screen_pause".into(),
                description: "Pause a video or a sound playing in a window; without a window — all that play.".into(),
                parameters: object(window(), &[]),
            },
            "pause",
            false,
        ),
        (
            LlmTool {
                name: "screen_resume".into(),
                description: "Resume a paused video or sound in a window; without a window — all that play.".into(),
                parameters: object(window(), &[]),
            },
            "resume",
            false,
        ),
    ]
}

fn run(screen: &Weak<Screen>, handle: &'static str) -> ToolRun {
    let screen = screen.clone();
    Arc::new(move |input: Map<String, Value>, _context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let screen = screen.clone();
        Box::pin(async move {
            let screen = screen.upgrade().ok_or_else(|| anyhow::anyhow!("the gateway is stopping"))?;
            let answer = screen.letter(handle, &Value::Object(input)).map_err(|error| anyhow::anyhow!(error))?;
            Ok(answer.to_string())
        })
    })
}

impl Connector for ScreenConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "screen".into(),
            kind: ConnectorKind::Lookup,
            title: crate::tr!("Экран"),
            description: crate::tr!("Окна на экране компьютера: какие открыты, оставить, закрыть, пауза"),
            cloud: false,
            env: Vec::new(),
            // It serves no capability of the routes: its tools are the brain's own, not a station's skill.
            capabilities: None,
            setup: None,
        }
    }

    fn listed(&self) -> bool {
        false
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move {
            match self.screen.upgrade() {
                Some(screen) if screen.available() => {
                    ConnectorStatus::ready(crate::tr!("Окна открывает приложение DuoWave"))
                }
                _ => ConnectorStatus::off(crate::tr!("Экрана нет")),
            }
        })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        if !self.screen.upgrade().is_some_and(|screen| screen.available()) {
            return Vec::new();
        }
        tools()
            .into_iter()
            .map(|(tool, handle, read_only)| ToolContribution {
                tool,
                permission: use_of(CapabilityName::Playback),
                run: run(&self.screen, handle),
                read_only,
                outside: false,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::roles::Role;

    #[tokio::test]
    async fn the_brain_closes_and_keeps_by_its_tools_only_where_there_is_a_screen() {
        let screen = Screen::new(None);
        let connector = ScreenConnector::new(&screen);
        assert!(connector.tools().is_empty());
        screen.set_available(true);
        let tools = connector.tools();
        assert_eq!(tools.len(), 5);
        screen
            .letter("open", &json!({ "items": [{ "kind": "video", "title": "Море", "file": "c".repeat(48) }] }))
            .unwrap();
        let context = || ToolContext {
            principal: household_principal(Role::Member, None),
            cancel: tokio_util::sync::CancellationToken::new(),
            station: None,
        };
        let keep = tools.iter().find(|tool| tool.tool.name == "screen_keep").unwrap();
        let input = json!({ "window": "море" }).as_object().cloned().unwrap();
        let answer = (keep.run)(input, context()).await.unwrap();
        assert!(answer.contains("\"kept\":true"), "{answer}");
        let close = tools.iter().find(|tool| tool.tool.name == "screen_close").unwrap();
        let input = json!({ "window": "all" }).as_object().cloned().unwrap();
        (close.run)(input, context()).await.unwrap();
        assert!(screen.windows().is_empty());
    }
}
