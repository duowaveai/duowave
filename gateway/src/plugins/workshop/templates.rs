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
    "desk.widgets": [{{ id: "main", title: {title_json}, icon: icons.sparkle, size: "small", initial: true, source: "плагин", Component: Widget }}],
  }},
}});
"#
    );
    vec![
        ("manifest.json".into(), pretty(&manifest)),
        ("backend.js".into(), backend),
        ("panel.js".into(), panel),
        ("README.md".into(), format!("# {title}\n\nВиджет на столе пульта: что он показывает и откуда берёт.\n")),
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
