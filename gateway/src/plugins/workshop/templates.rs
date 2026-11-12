//! What a new draft starts from: a plugin with a phrase and a tool that acts, or a theme with both palettes — files
//! that pass «Проверить» as they are, for the agent or a person to change.

use serde_json::json;

fn pretty(value: &serde_json::Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap_or_default())
}

/// A plugin: a phrase («скажи привет»), a tool for «Мозг», its store.
pub fn plugin(id: &str, title: &str) -> Vec<(String, String)> {
    let tool = format!("{}_hello", id.replace('-', "_"));
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Что он делает — для пульта",
        "api": "^1",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": ["voice.intents", "llm.tools", "kv"],
        "contributes": {
            "intents": [{ "phrases": ["скажи привет {name}".to_string(), "скажи привет".to_string()], "handler": "hello" }],
            "tools": [{
                "name": tool,
                "description": "Здоровается и считает, сколько раз поздоровался",
                "input": { "type": "object", "properties": { "name": { "type": "string", "description": "Кого приветствовать" } } },
                "handler": "hello",
                "annotations": { "readOnlyHint": false }
            }]
        }
    });
    let backend = format!(
        r#"// {title}: функции, которые зовёт шлюз (docs/plugins/AUTHORING.md). Ответ на фразу — строка (речь),
// {{say, play, listen}} или null («не моё»: ответит встроенный навык). Ответ инструмента — что угодно: «Мозг» читает
// его текстом; {{say, play}} — действие, шлюз сразу выполняет его.

export function hello({{ name }}) {{
  const count = (host.kv.get("count") ?? 0) + 1;
  host.kv.set("count", count);
  return `Привет, ${{name ?? "друг"}}! Это ${{count}}-й раз.`;
}}
"#
    );
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), backend),
        ("README.md".into(), format!("# {title}\n\nЧто умеет плагин и как его проверить голосом.\n")),
    ]
}

/// A quiz with the turn timer (docs/plans/plugin-schedules.md, stage 8): 10 seconds a move after the station has asked,
/// «время вышло» from its own function, the score in its store — what a game on time starts from.
pub fn quiz(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Викторина голосом: 10 секунд на ответ, колонка сама говорит «Время вышло!»",
        "api": "^1.2",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": ["voice.intents", "kv"],
        "contributes": { "intents": [{ "phrases": ["давай викторину", "викторина"], "handler": "start" }] }
    });
    let backend =
        r#"// __TITLE__: викторина с таймером хода (docs/plugins/AUTHORING.md, «Таймер хода»). На ход — SECONDS секунд
// после того, как колонка договорила вопрос: ответил — зовётся answer, промолчал — шлюз зовёт timeUp, и колонка сама
// говорит «Время вышло!». Две тишины подряд шлюз терпит, на третьей сам заканчивает разговор. Номер вопроса и счёт —
// в kv: память кода между ходами не живёт.

const QUESTIONS = [
  { ask: "Столица Австралии?", answer: "канберра" },
  { ask: "Сколько планет в Солнечной системе?", answer: "восемь" },
  { ask: "Самая длинная река в мире?", answer: "нил" },
];
const SECONDS = 10;
const STOP = /^(хватит|стоп|закончим|выход|надоело)$/;

const next = { handler: "answer", timeout: { seconds: SECONDS, handler: "timeUp" } };
const key = (request) => `quiz:${request?.device ?? "station"}`;
const plain = (text) => String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s]+/gu, " ").trim();

// Следующий вопрос — или итог, когда вопросы кончились (ответ строкой заканчивает разговор).
function ask(request, before) {
  const state = host.kv.get(key(request)) ?? { index: 0, score: 0 };
  if (state.index >= QUESTIONS.length) {
    host.kv.delete(key(request));
    return `${before} Вопросы кончились. Правильных ответов: ${state.score} из ${QUESTIONS.length}.`;
  }
  return { say: `${before} ${QUESTIONS[state.index].ask}`, continue: next };
}

// «Давай викторину».
export function start({ request }) {
  host.kv.set(key(request), { index: 0, score: 0 });
  return ask(request, `Викторина! На каждый ответ ${SECONDS} секунд.`);
}

// Ответ человека; timer.spent — сколько секунд он думал.
export function answer({ text, request, timer }) {
  const words = plain(text);
  if (STOP.test(words)) {
    host.kv.delete(key(request));
    return "Хорошо, доиграем в другой раз.";
  }
  const state = host.kv.get(key(request)) ?? { index: 0, score: 0 };
  const question = QUESTIONS[state.index];
  const right = Boolean(question) && words.includes(question.answer);
  host.kv.set(key(request), { index: state.index + 1, score: state.score + (right ? 1 : 0) });
  const quick = right && timer && timer.spent < 3 ? " Быстро!" : "";
  return ask(request, right ? `Верно!${quick}` : `Нет, это ${question?.answer ?? "другое"}.`);
}

// SECONDS секунд тишины; timeout.missed — какая это тишина подряд.
export function timeUp({ request, timeout }) {
  const state = host.kv.get(key(request)) ?? { index: 0, score: 0 };
  const question = QUESTIONS[state.index];
  host.kv.set(key(request), { ...state, index: state.index + 1 });
  const nudge = timeout.missed > 1 ? " Если вы тут — просто ответьте." : "";
  return ask(request, `Время вышло! Правильный ответ: ${question?.answer ?? "другой"}.${nudge}`);
}
"#
        .replace("__TITLE__", title);
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), backend),
        (
            "README.md".into(),
            format!(
                "# {title}\n\nВикторина голосом: «Алиса, давай викторину». На ответ 10 секунд; промолчали — колонка сама говорит «Время вышло!».\n"
            ),
        ),
    ]
}

/// A widget for the panel's desk (docs/plans/desk.md, stage 5): what it shows comes from its backend (`panelData`),
/// its part of the panel draws it (`panel.js`, the slot `desk.widgets`) — the frame, the name and the menu are the
/// desk's. The workshop's agent starts from it when someone asks for a widget in words. Its reading writes nothing:
/// every write to `kv` makes the panel's widgets read again, so a write in `panelData` would have them read every
/// second while the panel is open; its memory is written by a person's action (`mark`, the widget's button).
pub fn widget(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Виджет на столе пульта",
        "api": "^1",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 2, "storageMb": 1 },
        "permissions": ["kv"],
        "contributes": {
            "panel": { "slots": ["desk.widgets"], "calls": ["panelData", "mark"] }
        }
    });
    let backend = format!(
        r#"// {title}: данные виджета на столе пульта (docs/plugins/AUTHORING.md, «Виджет стола»). panelData и mark
// зовёт panel.js через useApi; ответ — любой JSON. Нужен интернет — право net:<хост> в манифесте; память между
// вызовами — host.kv.
//
// panelData только читает: каждая запись в host.kv — весть пульту, и все виджеты читают данные заново; запись при
// чтении крутила бы их раз в секунду. Кэш — только по сроку (пишется, когда устарел, как в weather); память, которую
// меняет человек, пишет функция его действия — здесь mark, кнопка виджета.

export function panelData() {{
  return {{ value: host.kv.get("marks") ?? 0, caption: "отметок" }};
}}

export function mark() {{
  const marks = (host.kv.get("marks") ?? 0) + 1;
  host.kv.set("marks", marks);
  return {{ value: marks, caption: "отметок" }};
}}
"#
    );
    // The title as a JavaScript string: what was asked in words may hold quotes.
    let title_json = serde_json::to_string(title).unwrap_or_else(|_| "\"Виджет\"".into());
    let panel = format!(
        r#"// {title}: виджет на столе пульта. Рамку — название, значок, меню «…», перетаскивание — даёт стол; здесь — только
// содержимое. Классы: w-big (крупное число), w-rows > w-row (строки), w-cap (подпись), w-day (плитка), w-btn (кнопка).
// version растёт, когда данные плагина изменились (запись в host.kv — голосом, кнопкой, из другого пульта): виджет
// читает их заново.
import {{ h }} from "preact";
import {{ useEffect, useState }} from "preact/hooks";
import {{ definePlugin, icons, useApi, usePanelState }} from "@duowave/panel-sdk";

function Widget() {{
  const api = useApi("{id}");
  const {{ version }} = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const failed = (e) => setError(e?.message ?? String(e));
  useEffect(() => {{
    api.call("panelData").then(setData).catch(failed);
  }}, [api, version]);
  const mark = () => {{
    setBusy(true);
    api.call("mark").then(setData).catch(failed).finally(() => setBusy(false));
  }};
  if (error) return h("p", {{ class: "w-cap m-0" }}, error);
  if (!data) return h("p", {{ class: "w-cap m-0" }}, "Читаю…");
  return h(
    "div",
    {{ class: "flex flex-col gap-6" }},
    h("span", {{ class: "w-big" }}, String(data.value)),
    h("span", {{ class: "w-cap" }}, data.caption),
    h("button", {{ class: "w-btn", type: "button", disabled: busy, onClick: mark }}, "Отметить"),
  );
}}

export default definePlugin({{
  id: "{id}",
  slots: {{
    "desk.widgets": [{{ id: "main", title: {title_json}, icon: icons.sparkle, size: "small", initial: true, source: "приложение", Component: Widget }}],
  }},
}});
"#
    );
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), backend),
        ("panel.js".into(), panel),
        // Stand-in answers of the network: before the person's «да» the tile shows these (sandbox.rs, «пример»).
        ("examples.json".into(), pretty(&json!({ "fetch": [] }))),
        ("README.md".into(), format!("# {title}\n\nВиджет на столе пульта: что он показывает и откуда берёт.\n")),
    ]
}

/// A teacher for a MIDI keyboard (docs/plans/midi.md, «Заготовка»): a widget on the desk and a lesson window — the
/// next note lit, a press sounds and lights the one after —, tunes of folk songs and old classics typed by hand, one's
/// own from a .mid file, the progress, phrases for the station. It runs on a computer: the keyboard is the gateway's
/// there (`runs: ["pc"]`, the right `midi`, Host API 1.3).
pub fn midi(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Учит играть на MIDI-клавиатуре: подсвечивает следующую ноту, звучит и ждёт верную",
        "api": "^1.3",
        "runs": ["pc"],
        "dataVersion": 1,
        "budget": { "memoryMb": 4, "storageMb": 4 },
        "permissions": ["midi", "kv", "voice.intents"],
        "settings": [{ "key": "names", "type": "boolean", "title": "Подписывать ноты", "default": true }],
        "contributes": {
            "panel": {
                "slots": ["desk.widgets"],
                "sections": [{ "id": id, "title": title }],
                "calls": ["state", "choose", "add", "remove", "keyboard", "done"]
            },
            "intents": [
                { "phrases": ["давай учить {melody}", "учим {melody}", "давай играть {melody}"], "handler": "learn" },
                { "phrases": ["какие мелодии есть", "что мы учим"], "handler": "about" }
            ]
        }
    });
    // The title as a JavaScript string where code reads it, as it is in comments: what was asked may hold quotes.
    let title_json = serde_json::to_string(title).unwrap_or_else(|_| "\"Учитель\"".into());
    let fill =
        |text: &str| text.replace("\"__TITLE__\"", &title_json).replace("__TITLE__", title).replace("__ID__", id);
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), fill(include_str!("templates/midi/backend.js"))),
        ("panel.js".into(), fill(include_str!("templates/midi/panel.js"))),
        (
            "README.md".into(),
            format!(
                "# {title}\n\nУчит играть на MIDI-клавиатуре, подключённой к компьютеру с DuoWave: подсвечивает следующую \
ноту, звучит и ждёт верную. Мелодии — народные и старая классика; свои — файлом .mid. Голосом: «Алиса, давай учить \
ёлочку», «какие мелодии есть». В «Примерке» — клавиатура на экране, своя подключится после «да».\n"
            ),
        ),
    ]
}

/// A theme: the tokens of «Минимал» it changes in both modes, the view it suggests and the density.
pub fn theme(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "theme",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Тема пульта",
        "api": "^1",
        "runs": ["pc", "station", "stationServer"],
        "contributes": {
            "themes": [{
                "id": id,
                "title": title,
                "about": "Чем она отличается — одной строкой",
                "modes": { "dark": "Тёмная", "light": "Светлая" },
                "layout": "desk",
                "density": 1,
                "palette": {
                    "dark": { "bg": "#0B120E", "accent": "#6EE7A8", "accent-text": "#6EE7A8" },
                    "light": { "bg": "#F1F7F3", "accent": "#1F9D63", "accent-text": "#137A4B" }
                }
            }]
        }
    });
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        (
            "README.md".into(),
            format!("# {title}\n\nТема пульта: токены «Стекла» поверх его палитры (docs/design/glass/THEMES.md).\n"),
        ),
    ]
}
