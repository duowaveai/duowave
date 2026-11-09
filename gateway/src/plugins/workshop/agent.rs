//! The workshop's agent: «Мозг» with the workshop's profile — the strong model, the built-in skill
//! (docs/plugins/skill/) as its instructions in place of the household's character, and the workshop's tools: a
//! draft's files, the sandbox's check, run and phrase, the plugin's journal, the install. The same tools are the
//! gateway's MCP server for Claude Code (routes.rs, `/mcp/workshop`), where each takes the draft's id.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::Workshop;
use crate::config::ModelProfile;
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::{DialogContext, DialogService, PhraseStream, ToolBox, ToolOutcome};
use crate::settings::definitions::{BrainPick, WORKSHOP_AGENT};

/// The skill: what the agent knows from the start.
pub const SKILL: &str = include_str!("../../../../docs/plugins/skill/SKILL.md");
/// The references it reads when it needs them (`docs`).
pub const AUTHORING: &str = include_str!("../../../../docs/plugins/AUTHORING.md");
pub const STATION: &str = include_str!("../../../../docs/plugins/skill/station.md");
pub const PANEL_SDK: &str = include_str!("../../../../docs/plugins/panel-sdk.d.ts");
pub const THEMES: &str = include_str!("../../../../docs/design/glass/THEMES.md");

/// Rounds of tool calls in one answer of the agent: writing and checking code takes many.
const ROUNDS: usize = 16;
/// The time for one answer's tool rounds.
const BUDGET_MS: u64 = 300_000;
/// What a tool's answer gives the model at most.
const ANSWER: usize = 24 * 1024;

/// The skill without its front matter (the lines between the first two `---`).
pub fn skill_text() -> &'static str {
    SKILL
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.trim_start())
        .unwrap_or(SKILL)
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

/// The workshop's tools. `with_draft`: each takes the draft's id (the MCP server); without it the chat is bound to one.
pub fn tools(with_draft: bool) -> Vec<LlmTool> {
    let draft = |mut properties: Value, required: &[&str]| {
        let mut required: Vec<&str> = required.to_vec();
        if with_draft {
            properties["draft"] =
                json!({ "type": "string", "description": "id черновика (строчные латиница, цифры, «-»)" });
            required.insert(0, "draft");
        }
        schema(properties, &required)
    };
    let mut tools = vec![
        LlmTool {
            name: "draft_files".into(),
            description: "Файлы черновика с размерами, вид пакета, версия и что стоит на шлюзе.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "read_file".into(),
            description: "Читает файл черновика: manifest.json, backend.js, panel.js, panel.css, README.md, SKILL.md или assets/….".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &["path"]),
        },
        LlmTool {
            name: "write_file".into(),
            description: "Записывает файл черновика целиком (создаёт или заменяет). После правки — check.".into(),
            parameters: draft(json!({ "path": { "type": "string" }, "content": { "type": "string" } }), &["path", "content"]),
        },
        LlmTool {
            name: "delete_file".into(),
            description: "Удаляет файл черновика.".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &["path"]),
        },
        LlmTool {
            name: "check".into(),
            description: "Проверяет черновик: манифест, загрузку кода в песочнице, что все функции на месте, что колонка не сыграет (http, AAC).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "run_function".into(),
            description: "Запускает функцию backend.js в песочнице с входом: результат, журнал, что сказала бы, хранилище.".into(),
            parameters: draft(
                json!({ "function": { "type": "string" }, "input": { "type": "object", "description": "Аргумент функции" } }),
                &["function"],
            ),
        },
        LlmTool {
            name: "try_phrase".into(),
            description: "Прогоняет фразу, как её услышит маршрутизатор: какая фраза манифеста подошла, слоты, что ушло бы колонке (речь, play, ссылки), предупреждения.".into(),
            parameters: draft(json!({ "text": { "type": "string", "description": "Например «Алиса, включи радио джаз»" } }), &["text"]),
        },
        LlmTool {
            name: "plugin_journal".into(),
            description: "Журнал поставленного плагина: его console, вызовы и ошибки, что колонка сказала о его станциях (началa играть, молчит).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "install".into(),
            description: "Ставит черновик новой версией рядом с прежней и перезапускает плагин; не поднялась — работает прежняя. Первую установку и новые права одобряет администратор в пульте.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "docs".into(),
            description: "Справочник: authoring — манифест, Host API, лимиты; station — что умеет колонка; panel — SDK пульта (panel.js); themes — контракт темы.".into(),
            parameters: schema(json!({ "topic": { "type": "string", "enum": ["authoring", "station", "panel", "themes"] } }), &["topic"]),
        },
    ];
    if with_draft {
        tools.push(LlmTool {
            name: "list_drafts".into(),
            description: "Черновики мастерской.".into(),
            parameters: schema(json!({}), &[]),
        });
        tools.push(LlmTool {
            name: "new_draft".into(),
            description: "Новый черновик: из заготовки (kind plugin или theme), из описания API (openapi — JSON документа OpenAPI) или из своего поставленного пакета с тем же id.".into(),
            parameters: schema(
                json!({
                    "draft": { "type": "string" },
                    "kind": { "type": "string", "enum": ["plugin", "theme"] },
                    "title": { "type": "string" },
                    "openapi": { "type": "string", "description": "Документ OpenAPI в JSON" }
                }),
                &["draft"],
            ),
        });
    }
    tools
}

fn text_of(value: &impl serde::Serialize) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    if text.len() <= ANSWER {
        return text;
    }
    let mut end = ANSWER;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Runs a workshop tool for a draft; its answer as the model reads it, and whether it failed.
pub async fn run_tool(workshop: &Workshop, draft: &str, name: &str, input: &Map<String, Value>) -> (String, bool) {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let answer: Result<String, String> = match name {
        "draft_files" => workshop.draft(draft).map(|view| text_of(&view)).map_err(|error| error.message),
        "read_file" => workshop.read(draft, &field("path")).map_err(|error| error.message),
        "write_file" => workshop
            .write(draft, &field("path"), &field("content"))
            .map(|_| format!("Записан {}. Теперь check.", field("path")))
            .map_err(|error| error.message),
        "delete_file" => {
            workshop.delete_file(draft, &field("path")).map(|_| "Удалён.".into()).map_err(|error| error.message)
        }
        "check" => workshop.check(draft).await.map(|report| text_of(&report)).map_err(|error| error.message),
        "run_function" => workshop
            .run(draft, &field("function"), input.get("input").cloned().unwrap_or_else(|| json!({})))
            .await
            .map(|report| text_of(&report))
            .map_err(|error| error.message),
        "try_phrase" => workshop
            .try_phrase(draft, &field("text"))
            .await
            .map(|report| text_of(&report))
            .map_err(|error| error.message),
        "plugin_journal" => Ok(text_of(&workshop.journal(draft))),
        // The agent never approves: the first install and new rights wait for the admin in the panel.
        "install" => {
            workshop.install(draft, false, None).await.map(|report| text_of(&report)).map_err(|error| error.message)
        }
        "docs" => Ok(match field("topic").as_str() {
            "station" => STATION.to_string(),
            "panel" => PANEL_SDK.to_string(),
            "themes" => THEMES.to_string(),
            _ => AUTHORING.to_string(),
        }),
        "list_drafts" => Ok(text_of(&workshop.drafts())),
        "new_draft" => {
            let kind = input.get("kind").and_then(Value::as_str);
            let title = input.get("title").and_then(Value::as_str);
            let openapi = input.get("openapi").and_then(Value::as_str);
            workshop.create(draft, kind, title, openapi).map(|view| text_of(&view)).map_err(|error| error.message)
        }
        other => Err(format!("нет инструмента {other}")),
    };
    match answer {
        Ok(text) => (text, false),
        Err(error) => (format!("error: {error}"), true),
    }
}

/// The agent's instructions: the skill, and the draft it works on now.
pub fn instructions(workshop: &Workshop, draft: &str) -> String {
    let now = match workshop.draft(draft) {
        Ok(view) => format!(
            "Черновик сейчас: «{}» ({}), версия {}, файлы: {}. Поставлено на шлюзе: {}.",
            view.title.clone().unwrap_or_else(|| draft.to_string()),
            view.kind.map(|kind| kind.as_str()).unwrap_or("?"),
            view.version.clone().unwrap_or_else(|| "?".into()),
            view.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>().join(", "),
            view.installed.clone().unwrap_or_else(|| "нет".into())
        ),
        Err(_) => format!("Черновика «{draft}» ещё нет."),
    };
    let placement = workshop.host().placement_now();
    format!(
        "{}\n\n## Сейчас\n\n{now} Шлюз работает: {} ({}). Отвечай по-русски, коротко и по делу: что сделал, что проверил, что \
дальше. Код показывай только если об этом просят — он в черновике.",
        skill_text(),
        placement.title(),
        placement.as_str()
    )
}

/// The agent's own model (`workshop.agent`): None — the smart profile of «Мозг».
pub fn model_of(workshop: &Workshop) -> Option<BrainPick> {
    workshop.host().settings().get(&WORKSHOP_AGENT, None)
}

/// What the agent answers with: the workshop's instructions, its own model or the smart profile.
pub fn context(workshop: &Workshop, draft: &str, model: Option<BrainPick>) -> DialogContext {
    DialogContext {
        profile: Some(ModelProfile::Smart),
        system: Some(instructions(workshop, draft)),
        model,
        ..DialogContext::default()
    }
}

/// One answer of the agent for a draft: phrases as the model writes them; its tool calls run on the workshop and are
/// told through `told` (name, input; then name, answer, failed).
pub fn chat(
    dialog: &Arc<DialogService>,
    workshop: Arc<Workshop>,
    key: &str,
    draft: &str,
    message: &str,
    told: Arc<dyn Fn(ChatStep) + Send + Sync>,
    cancel: CancellationToken,
) -> PhraseStream {
    let context = context(&workshop, draft, model_of(&workshop));
    let draft_id = draft.to_string();
    let mut toolbox = ToolBox::new(tools(false), move |call: ToolCall, _cancel: CancellationToken| {
        let (workshop, draft, told) = (workshop.clone(), draft_id.clone(), told.clone());
        async move {
            told(ChatStep::Tool { name: call.name.clone(), input: call.input.clone() });
            let (content, failed) = run_tool(&workshop, &draft, &call.name, &call.input).await;
            told(ChatStep::Result { name: call.name.clone(), content: content.chars().take(600).collect(), failed });
            Ok(if failed { ToolOutcome::error(content) } else { ToolOutcome::from(content) })
        }
    });
    toolbox.rounds = Some(ROUNDS);
    toolbox.budget_ms = Some(BUDGET_MS);
    dialog.answer(key, message, context, cancel, Some(toolbox))
}

/// What the agent did on the way, for the panel's chat.
#[derive(Debug, Clone)]
pub enum ChatStep {
    Tool { name: String, input: Map<String, Value> },
    Result { name: String, content: String, failed: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_the_agent_its_skill_and_every_tool_a_draft_or_none() {
        assert!(skill_text().starts_with("# Мастерская"), "the front matter is left out");
        assert!(AUTHORING.contains("## Host API 1.1") && STATION.contains("Проверено 07.11.2026"));
        let bound = tools(false);
        assert!(bound.iter().all(|tool| tool.parameters["required"]
            .as_array()
            .is_none_or(|required| !required.contains(&json!("draft")))));
        let mcp = tools(true);
        assert!(mcp.iter().any(|tool| tool.name == "new_draft"));
        let check = mcp.iter().find(|tool| tool.name == "check").unwrap();
        assert_eq!(check.parameters["required"], json!(["draft"]));
    }
}
