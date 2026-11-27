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
use crate::dialog::llm::{LlmTool, ToolCall, ToolCallCut};
use crate::dialog::{DialogContext, DialogService, PhraseStream, ToolBox, ToolOutcome};
use crate::plugins::history::{Author, Who};
use crate::settings::definitions::{BrainPick, WORKSHOP_AGENT};

/// The skill: what the agent knows from the start.
pub const SKILL: &str = include_str!("../../../../../docs/plugins/skill/SKILL.md");
/// The references it reads when it needs them (`docs`).
pub const AUTHORING: &str = include_str!("../../../../../docs/plugins/AUTHORING.md");
pub const STATION: &str = include_str!("../../../../../docs/plugins/skill/station.md");
pub const PANEL_SDK: &str = include_str!("../../../../../docs/plugins/panel-sdk.d.ts");
pub const CARDS: &str = include_str!("../../../../../docs/plugins/cards.md");
/// A window made to delight (a game): «Неоновая змейка», the built-in the agent learns windows from (`docs game`).
pub const GAME_PANEL: &str = include_str!("../../../../../packages/neon-snake/panel.js");
pub const GAME_STYLES: &str = include_str!("../../../../../packages/neon-snake/panel.css");
pub const GAME_MANIFEST: &str = include_str!("../../../../../packages/neon-snake/manifest.json");

/// Cards for the agent: the guide and the parts as the gateway knows them (src/bus/cards.rs) — one list for all.
pub fn cards_doc() -> String {
    let parts: Vec<String> =
        crate::bus::cards::PARTS.iter().map(|(name, what)| format!("- `{name}` — {what}")).collect();
    let scenes: Vec<String> =
        crate::bus::cards::SCENES.iter().map(|(name, what)| format!("- `{name}` — {what}")).collect();
    format!("{CARDS}\n{}\n\nСцены вида (`look.scene`):\n\n{}\n", parts.join("\n"), scenes.join("\n"))
}

/// A game window to learn from: what makes it, then the whole built-in «Неоновая змейка» — its manifest, panel.js and
/// panel.css.
pub fn game_doc() -> String {
    format!(
        "# Окошко-игра: пример «Неоновая змейка» (встроенная)\n\n\
Игра, своё рисование, живая графика — окошко (`panel.js`), не карточка. Что делает её красивой и живой:\n\n\
- Плитка стола со своей рамкой: `bare: true` в `desk.widgets`, корень с классом `widget`, свой фон и свет; на ней \
уже что-то движется (змейка играет сама), рядом рекорд и кнопка «Играть» → `usePanelState().go(id)`.\n\
- Сама игра — раздел `sections: [{{ id, title, icon, Screen, nav: \"link\" }}]`, его открывает `go`.\n\
- Холст по ширине окошка (высоту окошко берёт по содержимому): ширина × доля, с пределами; `devicePixelRatio`.\n\
- Один цикл `requestAnimationFrame` (60 кадров в игре, 30 на плитке), стоп при `document.hidden`.\n\
- Сети у окошка нет (`connect-src 'none'`): 3D и эффекты — своим кодом на canvas, без библиотек.\n\
- Управление: стрелки и WASD (окошко слышит клавиши в фокусе — подсказка «нажмите на поле»), свайпы, кнопки на \
экране; пробел — пауза; пауза, когда окошко теряет фокус.\n\
- Звук — WebAudio после первого нажатия, короткие ноты синтезом, кнопка «без звука».\n\
- Сочный вид поверх любой темы — фон, текст, кнопки и текст на них своими цветами, так игра одинакова в светлой и \
тёмной теме; свет, искры, тряска при ошибке; панели и кнопки поверх холста. Неон — вид этой змейки, а не правило: у \
своей игры вид по её теме и словам человека — светлый бумажный, пастельный, пиксельный, природный, журнальный…\n\
- Рекорд и данные — в `backend.js` через `useApi(id).call`: функция чтения ничего не пишет, пишет функция действия.\n\
- Тексты — словарём `{{ ru: {{…}}, en: {{…}} }}`: в компонентах `useT(T)`, подписи раздела и плитки в `definePlugin` — \
`t(T, key)` из SDK; в манифесте — `languages` и `locales.en` (`docs`, тема `authoring`, «Языки»).\n\n\
## manifest.json\n\n```json\n{GAME_MANIFEST}```\n\n## panel.js\n\n```js\n{GAME_PANEL}```\n\n## panel.css\n\n\
```css\n{GAME_STYLES}```\n"
    )
}
pub const THEMES: &str = include_str!("../../../../../docs/design/glass/THEMES.md");

/// Examples the agent picks from itself (`docs` «examples», owner 22.11: not every app grows out of one template): what
/// each shows, then, by `name`, its files. The model builds its own thing by the rules, looking at those close to the
/// request — never a copy of one.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("quiz", "игра голосом на колонке: вопросы, ходы, таймер «Время вышло!», счёт в kv"),
    ("games", "игры-разговоры, где ведёт модель (`model` + `continue`): без окошка"),
    ("snake", "окошко-игра «Неоновая змейка»: canvas, плитка со своей рамкой, раздел «Играть», клавиши, свайпы, звук"),
    ("midi", "окошко с уроком и клавиатурой: своё рисование, ввод тоньше кнопок, звук, только компьютер"),
    ("folders", "окошко-альбом из домашней папки: фото дня на карточке, листание в окошке"),
    ("widget", "простая карточка: число, подпись, кнопка, память в kv"),
    ("weather", "карточка с сетью: данные из интернета, пример ответа, живой вид по погоде и времени суток"),
    ("currency", "карточка с числами и линией из сети"),
    ("shopping", "список голосом и карточкой: фразы, инструменты для «Мышления», строки и кнопки"),
    ("plugin", "навык: фраза колонке и инструмент для модели"),
];

/// The `docs` «examples»: without a name — the list; with one — that example's files as they are.
pub fn examples_doc(name: &str) -> String {
    let files = |files: Vec<(String, String)>| {
        files.into_iter().map(|(path, text)| format!("## {path}\n\n```\n{text}```\n")).collect::<Vec<_>>().join("\n")
    };
    let built_in = |id: &str| {
        let found: Vec<(String, String)> = ["manifest.json", "backend.js", "panel.js", "panel.css"]
            .into_iter()
            .filter_map(|file| {
                crate::plugins::bundled::embedded_text(&format!("{id}/{file}")).map(|text| (file.to_string(), text))
            })
            .collect();
        files(found)
    };
    let body = match name.trim() {
        "quiz" => files(super::templates::quiz("example", "Викторина на время")),
        "midi" => files(super::templates::midi("example", "Учитель на MIDI-клавиатуре")),
        "folders" => files(super::templates::folders("example", "Фотоальбом")),
        "widget" => files(super::templates::widget("example", "Отметки")),
        "plugin" => files(super::templates::plugin("example", "Привет")),
        "snake" => return game_doc(),
        id @ ("games" | "weather" | "currency" | "shopping") => built_in(id),
        _ => {
            let list: Vec<String> = EXAMPLES.iter().map(|(name, what)| format!("- `{name}` — {what}")).collect();
            return format!(
                "# Примеры\n\nНе заготовки, а примеры: посмотри те, что ближе к просьбе (`docs` с topic «examples» и name), \
и собери своё по правилам — не копию одного. Вид — свой, не вид примера.\n\n{}\n",
                list.join("\n")
            );
        }
    };
    format!("# Пример «{}»\n\n{body}", name.trim())
}

/// Rounds of tool calls in one answer of the agent: an app written in parts and checked takes many.
const ROUNDS: usize = 40;
/// The time for one answer's tool rounds.
const BUDGET_MS: u64 = 30 * 60_000;
/// Times one answer's steps are asked again when the connection broke them (after 3, 10 and 30 s): about a minute
/// of a network gone.
const RETRIES: usize = 3;
/// One round may write nothing this long at the least: the model writes a file in one call, and a streamed call comes
/// only whole (LLM_TIMEOUT_MS, 10 s, cut such rounds off).
pub const ROUND_MS: u64 = 10 * 60_000;
/// A slow model's pace, milliseconds a token (25 tokens a second): a round that writes WORKSHOP_MAX_TOKENS whole may
/// take this long for each.
pub const MS_PER_TOKEN: u64 = 40;
/// What a tool's answer gives the model at most.
const ANSWER: usize = 24 * 1024;

/// How long one round may write nothing: a whole answer of `max_tokens` at a slow model's pace, ROUND_MS at the least.
pub fn round_ms(max_tokens: u32) -> u64 {
    (u64::from(max_tokens) * MS_PER_TOKEN).max(ROUND_MS)
}

/// Words of the person added to the work on the way, as the agent reads them (the background's rules say what they
/// are: make.rs, `BACKGROUND_RULES`).
pub const AFTER: &str = "Вдогонку: ";

/// What the agent hears when its turn ran out of tokens in a call (a whole file did not fit in one answer): the call
/// did not run, and the rounds go on.
pub fn cut_note(cut: &ToolCallCut) -> String {
    let file = cut.path.as_deref().map(|path| format!(" ({path})")).unwrap_or_default();
    format!(
        "Шлюз: твой прошлый ответ оборвался на вызове {}{file} — он не влез в предел длины ответа. Вызов не выполнен, \
файл не изменён. Пиши большой файл частями: первый кусок — write_file, следующие — append_file, по 100–200 строк за \
вызов; большой файл правь кусками через edit_file. Продолжай с того, что не записано.",
        cut.tool
    )
}

/// What the agent hears when its turn ran out of tokens before it called anything: it thought the whole thing through
/// or wrote the code in its words, and nothing was written (19.11, a game: 16 000 tokens, no file).
const CUT_ANSWER: &str = "Шлюз: твой прошлый ответ оборвался на пределе длины раньше, чем ты вызвал хоть один \
инструмент, — ничего не записано. Не продумывай всю работу разом и не пиши код в ответе: сразу записывай файлы \
инструментами, частями — первый кусок write_file, следующие append_file, по 100–200 строк за вызов. Начни с \
простого рабочего варианта, потом улучшай.";

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
                json!({ "type": "string", "description": "The draft's id: from list_drafts or new_draft's answer" });
            required.insert(0, "draft");
        }
        schema(properties, &required)
    };
    let mut tools = vec![
        LlmTool {
            name: "draft_files".into(),
            description: "The draft's files with sizes, the package kind, its version and what is installed on the gateway.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "read_file".into(),
            description: "Read a draft file: manifest.json, backend.js, panel.js, panel.css, README.md, SKILL.md or assets/….".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &["path"]),
        },
        LlmTool {
            name: "write_file".into(),
            description: "Write a whole draft file (create or replace). A big file (a panel.js window, a long backend.js) goes in parts: the first here, the next ones by append_file, 100–200 lines a call — a long file does not fit one answer. Change a big ready file with edit_file. Then check.".into(),
            parameters: draft(json!({ "path": { "type": "string" }, "content": { "type": "string" } }), &["path", "content"]),
        },
        LlmTool {
            name: "append_file".into(),
            description: "Append a part to the end of a draft file, on a new line (creates a missing file). A big file: the first part by write_file, the next ones here, 100–200 lines a call. After the last part — check.".into(),
            parameters: draft(
                json!({
                    "path": { "type": "string" },
                    "content": { "type": "string", "description": "The file's next part" }
                }),
                &["path", "content"],
            ),
        },
        LlmTool {
            name: "edit_file".into(),
            description: "Edit a draft file: replace old with new. old is an exact piece of the file (indentation too), found once; add lines around it if it repeats. One edit a call. Then check.".into(),
            parameters: draft(
                json!({
                    "path": { "type": "string" },
                    "old": { "type": "string", "description": "An exact piece found once in the file" },
                    "new": { "type": "string", "description": "What replaces it" }
                }),
                &["path", "old", "new"],
            ),
        },
        LlmTool {
            name: "delete_file".into(),
            description: "Delete a draft file.".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &["path"]),
        },
        LlmTool {
            name: "check".into(),
            description: "Check the draft: the manifest, the code loading in the sandbox, every function in place, what a speaker will not play (http, AAC).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "run_function".into(),
            description: "Run a backend.js function in the sandbox with an input: its result, log, what it would say, storage. example: true — network answers from examples.json, no network: so the tile and «Примерка» show the draft before «да».".into(),
            parameters: draft(
                json!({
                    "function": { "type": "string" },
                    "input": { "type": "object", "description": "The function's argument" },
                    "example": { "type": "boolean", "description": "On the example (examples.json), no network" }
                }),
                &["function"],
            ),
        },
        LlmTool {
            name: "try_phrase".into(),
            description: "Run a phrase as the router hears it: which manifest phrase matched, the slots, what would go to the speaker (speech, play, links), warnings. example: true — on the example, no network.".into(),
            parameters: draft(
                json!({
                    "text": { "type": "string", "description": "Like «Алиса, включи радио джаз»" },
                    "example": { "type": "boolean", "description": "On the example (examples.json), no network" }
                }),
                &["text"],
            ),
        },
        LlmTool {
            name: "plugin_journal".into(),
            description: "The installed plugin's journal: its console, calls and errors, what the speaker said of its streams (started playing, silent).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "install".into(),
            description: "Install the draft as a new version beside the old one and restart the plugin; if it does not come up, the old one works. The admin approves the first install and new rights in the panel.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "history".into(),
            description: "The draft's history, newest first: id, when, who, what was asked, version, labels (draft, installed).".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "show_version".into(),
            description: "A history record: its files, or one file (path) as it was then. record — the id or its first 7 characters.".into(),
            parameters: draft(json!({ "record": { "type": "string" }, "path": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "compare".into(),
            description: "The difference between a history record (from) and the draft now or another record (to): files, lines, rights and manifest phrases.".into(),
            parameters: draft(json!({ "from": { "type": "string" }, "to": { "type": "string" } }), &["from"]),
        },
        LlmTool {
            name: "restore_step".into(),
            description: "Bring the draft back to a history record as a new step, so it can be undone. «Верни как было до этого» — the step before the last.".into(),
            parameters: draft(json!({ "record": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "merge".into(),
            description: "Merge a history record into the draft: what merges by itself is in the draft; conflicts are yours (merge_state). For «Возьми из версии от вторника …».".into(),
            parameters: draft(json!({ "record": { "type": "string" } }), &["record"]),
        },
        LlmTool {
            name: "merge_state".into(),
            description: "The merge in progress: why you, the conflicting files and their three versions — base, ours (the draft), theirs. path — one file.".into(),
            parameters: draft(json!({ "path": { "type": "string" } }), &[]),
        },
        LlmTool {
            name: "finish_merge".into(),
            description: "Finish the merge: checks the draft and records a step with both sides. check first.".into(),
            parameters: draft(json!({}), &[]),
        },
        LlmTool {
            name: "ask_owner".into(),
            description: "Ask the owner when the meaning does not tell what to keep in a merge: a question and 2–4 options. The merge waits for the answer — do not guess.".into(),
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
            description: "Reference: authoring — the manifest, Host API, bus, limits; station — what the speaker can do; cards — widget cards, their looks (scenes, colours, photos) and when a window is needed; panel — the panel SDK (panel.js, a window); game — an example of a fine game window (all of «Неоновая змейка»; its neon is its own look, not a rule); examples — examples to build from, not templates: the list, and with `name` that example's files (quiz, games, snake, midi, folders, widget, weather, currency, shopping, plugin); themes — the theme contract: three colours of a mode, a backdrop from the panel's scenes, the palette.".into(),
            parameters: schema(
                json!({
                    "topic": { "type": "string", "enum": ["authoring", "station", "cards", "panel", "game", "themes", "examples"] },
                    "name": { "type": "string", "description": "examples only: which example's files" }
                }),
                &["topic"],
            ),
        },
    ];
    if with_draft {
        tools.push(LlmTool {
            name: "list_drafts".into(),
            description: "The workshop's drafts.".into(),
            parameters: schema(json!({}), &[]),
        });
        tools.push(LlmTool {
            name: "new_draft".into(),
            description: "A new draft: from a template (kind plugin; game — a blank game; quiz — a quiz with a turn timer «Время вышло!»; midi — a MIDI keyboard teacher; folders; theme), from an API description (openapi — an OpenAPI JSON document) or from an own installed package (draft — its id). The workshop gives the new draft's id — a random «p-» and 12 characters 0–9 a–f — in the answer (field id); call the tools with it. The name goes in title.".into(),
            parameters: schema(
                json!({
                    "draft": { "type": "string", "description": "Only to open an own installed package; none for a new one" },
                    "kind": { "type": "string", "enum": ["plugin", "game", "quiz", "midi", "folders", "theme"] },
                    "title": { "type": "string" },
                    "openapi": { "type": "string", "description": "An OpenAPI document in JSON" }
                }),
                &[],
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

/// A piece added at the end of a draft's file, on a new line; a file not there yet begins with it. A big file is
/// written so, in parts: whole, it may not fit in one answer of the model (the turn is cut off at its output limit in
/// the middle of the call, and the cut call is not run).
pub fn append(workshop: &Workshop, draft: &str, path: &str, piece: &str) -> Result<(), String> {
    if piece.is_empty() {
        return Err("content пустой: какой кусок дописать?".into());
    }
    let mut text = match workshop.read(draft, path) {
        Ok(text) => text,
        Err(error) if error.status == axum::http::StatusCode::NOT_FOUND => String::new(),
        Err(error) => return Err(error.message),
    };
    if !text.is_empty() && !text.ends_with('\n') && !piece.starts_with('\n') {
        text.push('\n');
    }
    text.push_str(piece);
    workshop.write(draft, path, &text).map(|_| ()).map_err(|error| error.message)
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
        "append_file" => append(workshop, draft, &field("path"), &field("content"))
            .map(|_| format!("Дописан {}. Дальше — следующий кусок или check.", field("path"))),
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
            "cards" => cards_doc(),
            "panel" => PANEL_SDK.to_string(),
            "game" => game_doc(),
            "themes" => THEMES.to_string(),
            "examples" => examples_doc(&field("name")),
            _ => AUTHORING.to_string(),
        }),
        "list_drafts" => Ok(text_of(&workshop.drafts())),
        "new_draft" => {
            let kind = input.get("kind").and_then(Value::as_str);
            let title = input.get("title").and_then(Value::as_str);
            let openapi = input.get("openapi").and_then(Value::as_str);
            // A package of one's own opens under its id; a new draft gets a random one.
            let own = kind.is_none() && openapi.is_none() && workshop.host().installed(draft).is_some();
            let id = if own { Ok(draft.to_string()) } else { workshop.new_draft_id(Some(draft)) };
            id.and_then(|id| workshop.create(&id, kind, title, openapi))
                .map(|view| text_of(&view))
                .map_err(|error| error.message)
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
    // The household's language (item 50: a widget asked for in Russian came out in English, its date American): the
    // texts people see are written in it first, the dates and numbers in its format; none set — the request's.
    let household = crate::locale::household(workshop.host().settings());
    let language = match household.as_deref() {
        Some(code) => format!("Язык дома — «{code}»"),
        None => "Язык дома не задан — бери язык просьбы".to_string(),
    };
    format!(
        "{}\n\n## Сейчас\n\n{now} Шлюз работает: {} ({}). {language}: на нём название, описание и все тексты для людей \
(первым в `languages`), английский — вторым; даты, время и числа — через `Intl` на языке дома (`host.i18n.locale`, \
`useLocale()`), не в американском виде. Отвечай на языке просьбы, коротко и по делу: что сделал, что проверил, что \
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
        // Its deals are the workshop's (src/router).
        seat: Some(crate::router::Seat::Workshop),
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
    // The agent's turns write files: their own, longer limit (WORKSHOP_MAX_TOKENS), not a spoken answer's.
    let max_tokens = dialog.config.workshop_max_tokens;
    context.max_tokens = Some(max_tokens);
    if let Some(rules) = how.rules {
        context.system = context.system.map(|system| format!("{system}\n\n{rules}"));
    }
    let run = workshop.activity().begin(draft, how.by, message, &cancel);
    // A model without tools writes no file a run could be judged by (echo, in tests): its check alone decides, as
    // before. With tools, a run that changed nothing is no «готово».
    if !dialog.tools_for(&context) {
        run.changed.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let draft_id = draft.to_string();
    let (tools_workshop, told_run, told) = (workshop.clone(), run.clone(), how.told);
    let offered: Vec<LlmTool> = tools(false).into_iter().filter(|tool| how.install || tool.name != "install").collect();
    let mut toolbox = ToolBox::new(offered, move |call: ToolCall, _cancel: CancellationToken| {
        let (workshop, draft, told, run) = (tools_workshop.clone(), draft_id.clone(), told.clone(), told_run.clone());
        async move {
            if let Some(told) = &told {
                told(ChatStep::Tool { name: call.name.clone(), input: call.input.clone() });
            }
            // The answer names its call: the activity pairs each answer with the call it is for.
            let asked = workshop.activity().call(&run, &call.name, &call.input);
            let (content, failed) = run_tool(&workshop, &draft, &call.name, &call.input).await;
            workshop.activity().answer(&run, asked, &call.name, &content, failed);
            if matches!(call.name.as_str(), "write_file" | "append_file" | "edit_file" | "delete_file") {
                workshop.tell(&draft, "edited");
            }
            // The run changed the draft: its end may be «готово» (a run that changed nothing is not).
            if !failed
                && matches!(
                    call.name.as_str(),
                    "write_file" | "append_file" | "edit_file" | "delete_file" | "restore_step" | "finish_merge"
                )
            {
                run.changed.store(true, std::sync::atomic::Ordering::Relaxed);
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
    toolbox.round_ms = Some(round_ms(max_tokens));
    toolbox.cut_note = Some(cut_note);
    toolbox.cut_answer_note = Some(CUT_ANSWER);
    // A file written in parts: the pieces of one turn go in their order.
    toolbox.in_order = true;
    // Long work outlives a network that blinks: a step the connection broke (14.11, «Anthropic stream broke … Operation
    // timed out» in the middle of a game) is asked again instead of failing the whole run.
    toolbox.retries = RETRIES;
    // Forty rounds over one growing history: each pays in full only for what is new (Claude's cache).
    toolbox.cache_history = true;
    // A last round forced by the limits: the run may have stopped half-way (the card's «не доделано»).
    toolbox.limited = Some(run.limited.clone());
    // The model's thinking took its answer whole again: the card says why it is not finished.
    toolbox.thought_away = Some(run.thought.clone());
    // Words added on the way go to the agent between its steps (docs/plans/audio-interface.md, «Вдогонку»).
    let (inbox_workshop, inbox_run) = (workshop.clone(), run.clone());
    toolbox.inbox = Some(Arc::new(move || {
        let notes = inbox_workshop.activity().take_notes(&inbox_run);
        (!notes.is_empty()).then(|| format!("{AFTER}{}", notes.join("\n")))
    }));
    let answer = dialog.answer(key, message, context, run.cancel.clone(), Some(toolbox));
    workshop.activity().watch(run, answer)
}

/// What the agent did on the way, for the panel's chat.
#[derive(Debug, Clone)]
pub enum ChatStep {
    Tool { name: String, input: Map<String, Value> },
    Result { name: String, content: String, failed: bool },
}
