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

/// A theme: «Стекло»'s tokens it changes in both modes, the layout and the density.
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
                "layout": "tabbar",
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
