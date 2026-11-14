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
        "api": "^2",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": [],
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
        "api": "^2",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": [],
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

/// A widget for the panel's desk (docs/plans/desk.md, stage 5; docs/plans/bus.md, «Карточки»): a card — its data
/// comes from its backend (`panelData`), the panel draws it with its own parts; no code of the app runs in the panel,
/// and the same card shows on a phone, in a chat, or said aloud. The workshop's agent starts from it when someone asks
/// for a widget in words; a window of its own (`panel.js`) only when a card is not enough (a game, drawing, a keyboard
/// on the screen). Its reading writes nothing: every write to `kv` makes the panel's widgets read again, so a write in
/// `panelData` would have them read every second while the panel is open; its memory is written by a person's action
/// (`mark`, the card's button).
pub fn widget(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Виджет на столе пульта",
        "api": "^2",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 2, "storageMb": 1 },
        "permissions": [],
        // Languages (docs/plans/i18n.md): the English title is the agent's to write — «Проверить» asks for it.
        "languages": ["ru", "en"],
        "locales": { "en": { "description": "A widget on the panel desk" } },
        "contributes": {
            "panel": {
                "calls": ["panelData", "mark"],
                "cards": [{ "id": "main", "title": title, "function": "panelData", "size": "small", "initial": true }]
            }
        }
    });
    let backend = format!(
        r#"// {title}: карточка на столе пульта (docs/plugins/cards.md). Функция карточки отдаёт её данные — детали,
// которые рисует пульт: big (крупное), cap (подпись), rows (строки), line (линия), tiles (плитки), buttons
// (кнопки)… и вид всего виджета — look: живая сцена пульта и цвета фона по смыслу (погода — небо за окном, музыка —
// волны, покупки — пузырьки). Нажатие кнопки зовёт функцию из contributes.panel.calls и получает новую карточку.
// Нужен интернет — право net:<хост> в манифесте и пример examples.json; память между вызовами — host.kv.
//
// panelData только читает: каждая запись в host.kv — весть пульту, и все виджеты читают данные заново; запись при
// чтении крутила бы их раз в секунду. Кэш — только по сроку (пишется, когда устарел, как в weather); память, которую
// меняет человек, пишет функция его действия — здесь mark, кнопка карточки.
//
// Тексты для людей — словарём по языкам (docs/plugins/AUTHORING.md, «Языки»): host.i18n.t берёт их на языке дома.

const T = {{
  ru: {{ marks: {{ one: "отметка", few: "отметки", many: "отметок" }}, mark: "Отметить", said: "Отметок: {{count}}." }},
  en: {{ marks: {{ one: "mark", other: "marks" }}, mark: "Mark", said: "Marks: {{count}}." }},
}};
const t = (key, params) => host.i18n.t(T, key, params);

function card(marks) {{
  return {{
    big: String(marks),
    cap: t("marks", {{ count: marks }}),
    buttons: [{{ text: t("mark"), tap: "mark" }}],
    look: {{ scene: "sparkles", colors: ['#5b2a91', '#c2477a', '#f39a5d'] }},
    say: t("said", {{ count: marks }}),
  }};
}}

export function panelData() {{
  return card(host.kv.get("marks") ?? 0);
}}

export function mark() {{
  const marks = (host.kv.get("marks") ?? 0) + 1;
  host.kv.set("marks", marks);
  return card(marks);
}}
"#
    );
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), backend),
        // Stand-in answers of the network: before the person's «да» the tile shows these (sandbox.rs, «пример»).
        ("examples.json".into(), pretty(&json!({ "fetch": [] }))),
        ("README.md".into(), format!("# {title}\n\nКарточка на столе пульта: что она показывает и откуда берёт.\n")),
    ]
}

/// A teacher for a MIDI keyboard (docs/plans/midi.md, «Заготовка»): a widget on the desk and a lesson window — the
/// next note lit, a press sounds and lights the one after —, tunes of folk songs and old classics typed by hand, one's
/// own from a .mid file, the progress, phrases for the station. It runs on a computer: the keyboard is the gateway's
/// there (`runs: ["pc"]`, the link `listen:device/midi`, Host API 2).
pub fn midi(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Учит играть на MIDI-клавиатуре: подсвечивает следующую ноту, звучит и ждёт верную",
        "api": "^2",
        "runs": ["pc"],
        "dataVersion": 1,
        "budget": { "memoryMb": 4, "storageMb": 4 },
        "permissions": ["listen:device/midi"],
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

/// A photo album (docs/plans/shared-folders.md, «Заготовка «Фотоальбом»»): the photo of the day from the home folder's
/// «Фото» as a large card, and a window with all of them in a grid. The link `call:files/home/Фото/*` — read «Фото»
/// with all in it, one «да»; the pictures come made small from the gateway (`picture: {address}`, `useFolderFile`).
/// Names aloud only with the folder's `speak`. A computer's: the home folder is there (`runs: ["pc"]`). In «Примерка»
/// the folder is the example's (`examples.json`, «bus»).
pub fn folders(id: &str, title: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": "Фото дня из папки «Фото» домашней папки DuoWave и окошко со всеми фотографиями",
        "api": "^2",
        "runs": ["pc"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": ["call:files/home/Фото/*"],
        "contributes": {
            "intents": [{ "phrases": ["покажи фото дня", "какое сегодня фото дня"], "handler": "todaySay" }],
            "panel": {
                "calls": ["today", "next", "page"],
                "cards": [{ "id": "today", "title": title, "function": "today", "size": "large", "initial": true, "icon": "sparkle" }]
            }
        }
    });
    let title_json = serde_json::to_string(title).unwrap_or_else(|_| "\"Фотоальбом\"".into());
    let fill =
        |text: &str| text.replace("\"__TITLE__\"", &title_json).replace("__TITLE__", title).replace("__ID__", id);
    // «Примерка» has no home folder: the example answers for «Фото» — two photos and a trip with one more.
    let photo = |address: &str, name: &str| json!({ "name": name, "address": address, "folder": false, "kind": "image", "size": 1 });
    let examples = json!({
        "fetch": [],
        "bus": [
            { "to": "files/home/Фото", "handle": "list", "result": { "items": [
                { "name": "Отпуск", "address": "files/home/Фото/Отпуск", "folder": true, "kind": "folder", "size": 0 },
                photo("files/home/Фото/Кот.jpg", "Кот.jpg"),
                photo("files/home/Фото/Ёлка.jpg", "Ёлка.jpg")
            ], "total": 3, "next": null } },
            { "to": "files/home/Фото/Отпуск", "handle": "list", "result": { "items": [
                photo("files/home/Фото/Отпуск/Море.jpg", "Море.jpg")
            ], "total": 1, "next": null } },
            { "to": "files/home/Фото", "handle": "stat", "result": { "name": "Фото", "folder": true, "speak": true, "model": false } }
        ]
    });
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), fill(include_str!("templates/folders/backend.js"))),
        ("panel.js".into(), fill(include_str!("templates/folders/panel.js"))),
        ("examples.json".into(), pretty(&examples)),
        (
            "README.md".into(),
            format!(
                "# {title}\n\nФото дня из папки «Фото» домашней папки DuoWave (`~/DuoWave/Фото`) — большая карточка на \
столе, «Другое» — следующее; окошко со всеми фото сеткой. Голосом: «Алиса, покажи фото дня». Право — читать «Фото» \
(`call:files/home/Фото/*`): «да» один раз. В «Примерке» — фото из примера.\n"
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
        "api": "^2",
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
