//! The workshop's agent: «Мозг» with the workshop's profile — the strong model, the built-in skill
//! (docs/plugins/skill/) as its instructions in place of the household's character, and the workshop's tools: a
//! draft's files, the sandbox's check, run and phrase, the plugin's journal, the install. The same tools are the
//! gateway's MCP server for Claude Code (routes.rs, `/mcp/workshop`), where each takes the draft's id.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::activity::By;
use super::Workshop;
use crate::config::ModelProfile;
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::{DialogContext, DialogService, PhraseStream, ToolBox, ToolOutcome};
use crate::plugins::history::{Author, Who};
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
/// One round may write nothing this long: the model writes a whole file in one call, and a streamed call comes only
/// whole (LLM_TIMEOUT_MS, 10 s, cut such rounds off).
const ROUND_MS: u64 = 120_000;
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
            description: "Записывает файл черновика целиком (создаёт или заменяет): для нового или маленького файла. Большой файл (заготовки, panel.js) правь edit_file — длинный ответ модель не допишет. После правки — check.".into(),
            parameters: draft(json!({ "path": { "type": "string" }, "content": { "type": "string" } }), &["path", "content"]),
        },
        LlmTool {
            name: "edit_file".into(),
            description: "Правит файл черновика куском: заменяет old на new. old — точный кусок файла (с отступами), один на весь файл; добавь строки вокруг, если он повторяется. Несколько правок — несколько вызовов. После правок — check.".into(),
            parameters: draft(
                json!({
                    "path": { "type": "string" },
                    "old": { "type": "string", "description": "Точный кусок, который есть в файле один раз" },
                    "new": { "type": "string", "description": "Чем заменить" }
                }),
                &["path", "old", "new"],
            ),
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
            description: "Запускает функцию backend.js в песочнице с входом: результат, журнал, что сказала бы, хранилище. example: true — ответы сети из examples.json, в сеть ни шагу: так плитка и «Примерка» показывают черновик до «да».".into(),
            parameters: draft(
                json!({
                    "function": { "type": "string" },
                    "input": { "type": "object", "description": "Аргумент функции" },
                    "example": { "type": "boolean", "description": "На примере (examples.json), без сети" }
                }),
                &["function"],
            ),
        },
        LlmTool {
            name: "try_phrase".into(),
            description: "Прогоняет фразу, как её услышит маршрутизатор: какая фраза манифеста подошла, слоты, что ушло бы колонке (речь, play, ссылки), предупреждения. example: true — на примере, без сети.".into(),
            parameters: draft(
                json!({
                    "text": { "type": "string", "description": "Например «Алиса, включи радио джаз»" },
                    "example": { "type": "boolean", "description": "На примере (examples.json), без сети" }
                }),
                &["text"],
            ),
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
            name: "history".into(),
            description: "История черновика: шаги и версии, новые сверху — id, когда, кто, что просили, версия, метки (draft — черновик, installed — поставлено).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "show_version".into(),
            description: "Запись истории: её файлы, или один файл (path) — как он был в той записи. record — id или его начало (7 знаков).".into(),
            parameters: draft(json!({ "record": { "type": "string" }, "path": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "compare".into(),
            description: "Разница между записью истории (from) и черновиком сейчас или другой записью (to): файлы, строки, права и фразы манифеста.".into(),
            parameters: draft(json!({ "from": { "type": "string" }, "to": { "type": "string" } }), &["from"]),
        },
        LlmTool {
            name: "restore_step".into(),
            description: "Возвращает черновик к записи истории — новым шагом, так что можно вернуться обратно. «Верни как было до этого» — к шагу перед последним.".into(),
            parameters: draft(json!({ "record": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "merge".into(),
            description: "Объединяет запись истории с черновиком: что объединилось само — уже в черновике; спорное — тебе (merge_state). «Возьми из версии от вторника …» — так.".into(),
            parameters: draft(json!({ "record": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "merge_state".into(),
            description: "Объединение, которое идёт: почему ты, спорные файлы и их три версии — основа, наша (черновик), другая. path — один файл.".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &[]),
        },
        LlmTool {
            name: "finish_merge".into(),
            description: "Заканчивает объединение: проверяет черновик и записывает шаг с обеими сторонами. Сначала check.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "ask_owner".into(),
            description: "Вопрос владельцу, когда по смыслу непонятно, что оставить при объединении: вопрос и 2–4 варианта. Объединение ждёт ответа — не угадывай.".into(),
            parameters: draft(
                json!({
                    "question": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } }
                }),
                &["question", "options"],
            ),
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
            description: "Новый черновик: из заготовки (kind plugin, quiz — викторина с таймером хода «Время вышло!», midi — учитель на MIDI-клавиатуре, — или theme), из описания API (openapi — JSON документа OpenAPI) или из своего поставленного пакета с тем же id.".into(),
            parameters: schema(
                json!({
                    "draft": { "type": "string" },
                    "kind": { "type": "string", "enum": ["plugin", "quiz", "midi", "theme"] },
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

/// A piece of a draft's file replaced: `old` must be there exactly once — a model that rewrites a big file whole runs
/// out of its answer (Claude stops at its token limit in the middle of the call, and the cut call is not run).
pub fn edit(workshop: &Workshop, draft: &str, path: &str, old: &str, new: &str) -> Result<(), String> {
    if old.is_empty() {
        return Err("old пустой: какой кусок заменить? Новый файл — write_file.".into());
    }
    let text = workshop.read(draft, path).map_err(|error| error.message)?;
    match text.matches(old).count() {
        0 => Err(format!("В {path} нет такого куска: прочитай файл (read_file) и возьми кусок точно, с отступами.")),
        1 => workshop.write(draft, path, &text.replacen(old, new, 1)).map(|_| ()).map_err(|error| error.message),
        many => Err(format!("В {path} такой кусок встречается {many} раз: добавь строки вокруг, чтобы он был один.")),
    }
}

/// Runs a workshop tool for a draft; its answer as the model reads it, and whether it failed.
pub async fn run_tool(workshop: &Workshop, draft: &str, name: &str, input: &Map<String, Value>) -> (String, bool) {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let trying = match input.get("example").and_then(Value::as_bool) {
        Some(true) => super::Trying::example(),
        _ => super::Trying::default(),
    };
    let answer: Result<String, String> = match name {
        "draft_files" => workshop.draft(draft).map(|view| text_of(&view)).map_err(|error| error.message),
        "read_file" => workshop.read(draft, &field("path")).map_err(|error| error.message),
        "write_file" => workshop
            .write(draft, &field("path"), &field("content"))
            .map(|_| format!("Записан {}. Теперь check.", field("path")))
            .map_err(|error| error.message),
        "edit_file" => edit(workshop, draft, &field("path"), &field("old"), &field("new"))
            .map(|_| format!("Поправлен {}. Теперь check.", field("path"))),
        "delete_file" => {
            workshop.delete_file(draft, &field("path")).map(|_| "Удалён.".into()).map_err(|error| error.message)
        }
        "check" => workshop.check(draft).await.map(|report| text_of(&report)).map_err(|error| error.message),
        "run_function" => workshop
            .run(draft, &field("function"), input.get("input").cloned().unwrap_or_else(|| json!({})), &trying)
            .await
            .map(|report| text_of(&report))
            .map_err(|error| error.message),
        "try_phrase" => workshop
            .try_phrase(draft, &field("text"), &trying)
            .await
            .map(|report| text_of(&report))
            .map_err(|error| error.message),
        "plugin_journal" => Ok(text_of(&workshop.journal(draft))),
        // The agent never approves: the first install and new rights wait for the admin in the panel.
        "install" => workshop
            .install(draft, false, &Map::new(), None)
            .await
            .map(|report| text_of(&report))
            .map_err(|error| error.message),
        "history" => history_text(workshop, draft),
        "show_version" => show_version(workshop, draft, &field("record"), input.get("path").and_then(Value::as_str)),
        "compare" => compare_text(workshop, draft, &field("from"), input.get("to").and_then(Value::as_str)),
        "restore_step" => workshop
            .restore_step(draft, &field("record"), Author { who: Who::Agent, name: None, via: None })
            .map(|_| format!("Черновик возвращён к записи {}. Теперь check.", field("record")))
            .map_err(|error| error.message),
        "merge" => merge_tool(workshop, draft, &field("record")).await,
        "merge_state" => workshop
            .merge_files(draft, input.get("path").and_then(Value::as_str))
            .map(|state| text_of(&state))
            .map_err(|error| error.message),
        "finish_merge" => workshop.finish_merge(draft).await.map_err(|error| error.message),
        "ask_owner" => {
            let options: Vec<String> = input
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            workshop.ask_owner(draft, &field("question"), options).map_err(|error| error.message)
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

/// The draft's history as the agent reads it: a line a record.
fn history_text(workshop: &Workshop, draft: &str) -> Result<String, String> {
    let history = workshop.history().ok_or("истории здесь нет")?;
    let feed = history.feed(draft, None, 40);
    if feed.records.is_empty() {
        return Ok("Истории у черновика ещё нет.".into());
    }
    let lines: Vec<String> = feed
        .records
        .iter()
        .map(|view| {
            let record = &view.record;
            let mut line = format!(
                "{} {} {:?} ({:?})",
                view.short,
                crate::plugins::history::restore::local_time(&record.at),
                record.kind,
                record.by.who
            );
            if let Some(version) = &record.version {
                line.push_str(&format!(" v{version}"));
            }
            if let Some(asked) = &record.asked {
                line.push_str(&format!(" просили: «{asked}»"));
            }
            if let Some(note) = &record.note {
                line.push_str(&format!(" — {note}"));
            }
            if !view.marks.is_empty() {
                line.push_str(&format!(" [{}]", view.marks.join(", ")));
            }
            line
        })
        .collect();
    Ok(text_of(&lines))
}

fn show_version(workshop: &Workshop, draft: &str, record: &str, path: Option<&str>) -> Result<String, String> {
    let history = workshop.history().ok_or("истории здесь нет")?;
    let id = history.resolve(draft, record).ok_or("такой записи нет")?;
    let found = history.record(&id).ok_or("такой записи нет")?;
    match path {
        Some(path) => {
            let bytes = history.file_at(&found.tree, path).ok_or_else(|| format!("в той записи нет {path}"))?;
            String::from_utf8(bytes).map_err(|_| "файл не текстовый".to_string())
        }
        None => Ok(text_of(&serde_json::json!({ "record": found, "files": history.files_of(&id) }))),
    }
}

fn compare_text(workshop: &Workshop, draft: &str, from: &str, to: Option<&str>) -> Result<String, String> {
    let history = workshop.history().ok_or("истории здесь нет")?;
    let from = history.resolve(draft, from).ok_or("такой записи нет")?;
    let to = match to.filter(|to| !to.is_empty()) {
        Some(to) => Some(history.resolve(draft, to).ok_or("такой записи нет")?),
        None => None,
    };
    history.compare(draft, Some(&from), to.as_deref()).map(|comparison| text_of(&comparison))
}

async fn merge_tool(workshop: &Workshop, draft: &str, record: &str) -> Result<String, String> {
    let history = workshop.history().ok_or("истории здесь нет")?;
    let id = history.resolve(draft, record).ok_or("такой записи нет")?;
    let by = Author { who: Who::Agent, name: None, via: None };
    let report = workshop.merge(draft, &id, false, by, None).await.map_err(|error| error.message)?;
    Ok(match report.merge {
        Some(merge) => workshop.merge_brief(&merge),
        None => report.message,
    })
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
    let how = Work { by: By::Panel, rules: None, install: true, told: Some(told) };
    work(dialog, workshop, key, draft, message, how, cancel)
}

/// How a run of the agent goes: who asked, the rules besides the skill (the background's), whether it may install
/// (never for a station: a voice can be faked), who hears its calls besides the activity.
pub struct Work {
    pub by: By,
    pub rules: Option<&'static str>,
    pub install: bool,
    pub told: Option<Arc<dyn Fn(ChatStep) + Send + Sync>>,
}

/// One run of the agent on a draft, its steps in the activity (the panel's plaque, the workshop's log, the draft's
/// card); «Остановить» ends it.
pub fn work(
    dialog: &Arc<DialogService>,
    workshop: Arc<Workshop>,
    key: &str,
    draft: &str,
    message: &str,
    how: Work,
    cancel: CancellationToken,
) -> PhraseStream {
    let mut context = context(&workshop, draft, model_of(&workshop));
    if let Some(rules) = how.rules {
        context.system = context.system.map(|system| format!("{system}\n\n{rules}"));
    }
    let run = workshop.activity().begin(draft, how.by, message, &cancel);
    let draft_id = draft.to_string();
    let (tools_workshop, told_run, told) = (workshop.clone(), run.clone(), how.told);
    let offered: Vec<LlmTool> = tools(false).into_iter().filter(|tool| how.install || tool.name != "install").collect();
    let mut toolbox = ToolBox::new(offered, move |call: ToolCall, _cancel: CancellationToken| {
        let (workshop, draft, told, run) = (tools_workshop.clone(), draft_id.clone(), told.clone(), told_run.clone());
        async move {
            if let Some(told) = &told {
                told(ChatStep::Tool { name: call.name.clone(), input: call.input.clone() });
            }
            // The calls of one turn run at once: the answer names its call.
            let asked = workshop.activity().call(&run, &call.name, &call.input);
            let (content, failed) = run_tool(&workshop, &draft, &call.name, &call.input).await;
            workshop.activity().answer(&run, asked, &call.name, &content, failed);
            if matches!(call.name.as_str(), "write_file" | "edit_file" | "delete_file") {
                workshop.tell(&draft, "edited");
            }
            if let Some(told) = &told {
                told(ChatStep::Result {
                    name: call.name.clone(),
                    content: content.chars().take(600).collect(),
                    failed,
                });
            }
            Ok(if failed { ToolOutcome::error(content) } else { ToolOutcome::from(content) })
        }
    });
    toolbox.rounds = Some(ROUNDS);
    toolbox.budget_ms = Some(BUDGET_MS);
    toolbox.round_ms = Some(ROUND_MS);
    let answer = dialog.answer(key, message, context, run.cancel.clone(), Some(toolbox));
    workshop.activity().watch(run, answer)
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
        assert!(AUTHORING.contains("## Host API 1.2") && STATION.contains("Проверено 07.11.2026"));
        let bound = tools(false);
        assert!(bound.iter().all(|tool| tool.parameters["required"]
            .as_array()
            .is_none_or(|required| !required.contains(&json!("draft")))));
        let mcp = tools(true);
        assert!(mcp.iter().any(|tool| tool.name == "new_draft"));
        let check = mcp.iter().find(|tool| tool.name == "check").unwrap();
        assert_eq!(check.parameters["required"], json!(["draft"]));
    }

    #[tokio::test]
    async fn edits_a_piece_of_a_big_file_and_says_what_is_wrong_with_a_piece_not_there_or_twice() {
        use crate::config::{Config, Env};
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let workshop = &app.workshop;
        workshop.create("uchitel", Some("midi"), None, None).unwrap();
        let input =
            |old: &str, new: &str| json!({ "path": "panel.js", "old": old, "new": new }).as_object().cloned().unwrap();
        let (said, failed) = run_tool(
            workshop,
            "uchitel",
            "edit_file",
            &input("const SIZES = [25, 32, 37, 49, 61, 76, 88];", "const SIZES = [25, 61];"),
        )
        .await;
        assert!(!failed && said.starts_with("Поправлен panel.js"), "{said}");
        assert!(workshop.read("uchitel", "panel.js").unwrap().contains("const SIZES = [25, 61];"));
        let (said, failed) = run_tool(workshop, "uchitel", "edit_file", &input("нет такого куска", "x")).await;
        assert!(failed && said.contains("нет такого куска"), "{said}");
        let (said, failed) = run_tool(workshop, "uchitel", "edit_file", &input("h(", "h (")).await;
        assert!(failed && said.contains("раз"), "{said}");
        assert!(tools(true).iter().any(|tool| tool.name == "edit_file"));
    }
}
