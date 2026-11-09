//! A draft from an API's description (`duowave plugin new --from-openapi`, the workshop's «Из OpenAPI»): each operation
//! of an OpenAPI 3 (or Swagger 2) document in JSON becomes a tool for «Мозг» in MCP's form — its name, its summary,
//! a JSON Schema of its parameters — and one function of backend.js that calls it through `host.http.fetch`. The
//! server's host becomes the plugin's only `net:` right; an API key the document asks for becomes a setting, never a
//! line of code. The agent then adds phrases and a card, and keeps the tools that matter.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

/// Tools a plugin may give (manifest.rs).
const MAX_TOOLS: usize = 20;
const METHODS: &[&str] = &["get", "post", "put", "patch", "delete"];

/// An operation, as backend.js calls it.
struct Operation {
    name: String,
    description: String,
    method: String,
    path: String,
    query: Vec<String>,
    path_params: Vec<String>,
    body: bool,
    input: Value,
}

/// `getWeatherForecast`, `/weather/{city}` → `get_weather_forecast`: lower case, `_`, a letter first, 48 at most.
fn tool_name(raw: &str) -> String {
    let mut name = String::new();
    let mut previous_lower = false;
    for c in raw.chars() {
        if c.is_ascii_uppercase() {
            if previous_lower {
                name.push('_');
            }
            name.push(c.to_ascii_lowercase());
            previous_lower = false;
        } else if c.is_ascii_alphanumeric() {
            name.push(c);
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !name.ends_with('_') {
                name.push('_');
            }
            previous_lower = false;
        }
    }
    let name = name.trim_matches('_').to_string();
    let name = if name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) { name } else { format!("op_{name}") };
    name.chars().take(48).collect::<String>().trim_end_matches('_').to_string()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_string)
}

/// A parameter's schema, kept simple: type, description, enum.
fn parameter_schema(parameter: &Value) -> Value {
    let schema = parameter.get("schema").unwrap_or(parameter);
    let mut out = Map::new();
    let kind = schema.get("type").and_then(Value::as_str).unwrap_or("string");
    out.insert(
        "type".into(),
        json!(if ["string", "number", "integer", "boolean"].contains(&kind) { kind } else { "string" }),
    );
    if let Some(description) = text(parameter, "description") {
        out.insert("description".into(), json!(description.chars().take(200).collect::<String>()));
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        out.insert("enum".into(), Value::Array(options.iter().take(30).cloned().collect()));
    }
    Value::Object(out)
}

/// The server's address: OpenAPI 3's first server, or Swagger 2's host and base path; https only.
fn server(spec: &Value) -> Result<url::Url, String> {
    let address = match spec.pointer("/servers/0/url").and_then(Value::as_str) {
        Some(address) => address.to_string(),
        None => {
            let host = spec.get("host").and_then(Value::as_str).ok_or("в описании нет servers[0].url (или host)")?;
            format!("https://{host}{}", spec.get("basePath").and_then(Value::as_str).unwrap_or(""))
        }
    };
    let url = url::Url::parse(&address).map_err(|_| format!("адрес сервера «{address}» не читается"))?;
    if url.scheme() != "https" {
        return Err(format!("сервер {address}: плагин ходит в сеть только по https"));
    }
    if url.host_str().is_none_or(|host| host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.')) {
        return Err("у сервера нет имени в интернете".into());
    }
    Ok(url)
}

/// The API key the document asks for, if any: where it goes and under which name.
fn api_key(spec: &Value) -> Option<(String, String)> {
    let schemes =
        spec.pointer("/components/securitySchemes").or_else(|| spec.get("securityDefinitions"))?.as_object()?;
    schemes.values().find_map(|scheme| {
        (scheme.get("type").and_then(Value::as_str) == Some("apiKey")).then(|| {
            (
                text(scheme, "in").unwrap_or_else(|| "header".into()),
                text(scheme, "name").unwrap_or_else(|| "X-API-Key".into()),
            )
        })
    })
}

fn operations(spec: &Value) -> Vec<Operation> {
    let mut found = Vec::new();
    let mut names = BTreeSet::new();
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else { return found };
    for (path, item) in paths {
        let shared: Vec<Value> = item.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
        for method in METHODS {
            let Some(operation) = item.get(*method) else { continue };
            if found.len() >= MAX_TOOLS {
                return found;
            }
            let raw = text(operation, "operationId").unwrap_or_else(|| format!("{method} {path}"));
            let mut name = tool_name(&raw);
            while !names.insert(name.clone()) {
                name = format!("{}_{}", name.chars().take(44).collect::<String>(), names.len());
            }
            let description = text(operation, "summary")
                .or_else(|| text(operation, "description"))
                .unwrap_or_else(|| format!("{} {path}", method.to_uppercase()))
                .chars()
                .take(300)
                .collect();
            let parameters: Vec<Value> = shared
                .iter()
                .chain(operation.get("parameters").and_then(Value::as_array).into_iter().flatten())
                .cloned()
                .collect();
            let mut properties = Map::new();
            let mut required = Vec::new();
            let (mut query, mut path_params) = (Vec::new(), Vec::new());
            for parameter in &parameters {
                let Some(name) = text(parameter, "name") else { continue };
                match parameter.get("in").and_then(Value::as_str) {
                    Some("query") => query.push(name.clone()),
                    Some("path") => path_params.push(name.clone()),
                    _ => continue,
                }
                properties.insert(name.clone(), parameter_schema(parameter));
                if parameter.get("required").and_then(Value::as_bool) == Some(true) {
                    required.push(json!(name));
                }
            }
            let body = operation.pointer("/requestBody/content/application~1json").is_some();
            if body {
                properties.insert("body".into(), json!({ "type": "object", "description": "Тело запроса (JSON)" }));
            }
            found.push(Operation {
                name,
                description,
                method: method.to_uppercase(),
                path: path.clone(),
                query,
                path_params,
                body,
                input: json!({ "type": "object", "properties": properties, "required": required }),
            });
        }
    }
    found
}

/// The handler's name for a tool: camelCase of its name.
fn handler_of(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The draft's files from a document in JSON.
pub fn draft_from(id: &str, title: Option<&str>, document: &str) -> Result<Vec<(String, String)>, String> {
    let spec: Value = serde_json::from_str(document).map_err(|_| "нужен JSON (YAML переведите в JSON)".to_string())?;
    let server = server(&spec)?;
    let host = server.host_str().unwrap_or_default().to_ascii_lowercase();
    let base = server.as_str().trim_end_matches('/').to_string();
    let operations = operations(&spec);
    if operations.is_empty() {
        return Err("в описании нет операций (paths)".into());
    }
    let title =
        title.map(str::to_string).or_else(|| spec.pointer("/info/title").and_then(Value::as_str).map(str::to_string));
    let title: String = title.unwrap_or_else(|| id.to_string()).chars().take(80).collect();
    let key = api_key(&spec);
    let tools: Vec<Value> = operations
        .iter()
        .map(|operation| {
            json!({
                "name": operation.name,
                "description": operation.description,
                "input": operation.input,
                "handler": handler_of(&operation.name),
                "annotations": { "readOnlyHint": operation.method == "GET" }
            })
        })
        .collect();
    let mut settings = Vec::new();
    if key.is_some() {
        settings.push(json!({ "key": "apiKey", "type": "string", "title": "Ключ API", "description": "Выдаёт сервис; хранится в настройках шлюза, не в коде" }));
    }
    let manifest = json!({
        "schema": 1,
        "kind": "plugin",
        "id": id,
        "version": "0.1.0",
        "title": title,
        "description": spec.pointer("/info/description").and_then(Value::as_str).unwrap_or("Инструменты API для «Мозга»").chars().take(300).collect::<String>(),
        "api": "^1",
        "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 4, "storageMb": 1 },
        "permissions": [format!("net:{host}"), "llm.tools".to_string()],
        "settings": settings,
        "contributes": { "tools": tools }
    });
    let table: Map<String, Value> = operations
        .iter()
        .map(|operation| {
            (
                operation.name.clone(),
                json!({ "method": operation.method, "path": operation.path, "query": operation.query, "pathParams": operation.path_params, "body": operation.body }),
            )
        })
        .collect();
    let auth = match &key {
        Some((place, name)) if place == "query" => format!(
            "  const key = host.settings.get(\"apiKey\");\n  if (key) url.searchParams.set({}, key);\n",
            json!(name)
        ),
        Some((_, name)) => {
            format!("  const key = host.settings.get(\"apiKey\");\n  if (key) headers[{}] = key;\n", json!(name))
        }
        None => String::new(),
    };
    let exports: String = operations
        .iter()
        .map(|operation| {
            format!(
                "export const {} = (input) => call({}, input);\n",
                handler_of(&operation.name),
                json!(operation.name)
            )
        })
        .collect();
    let backend = format!(
        r#"// {title}: инструменты «Мозга» из описания API (OpenAPI). Каждая функция зовёт свою операцию через host.http.fetch —
// только к {host} (право net:{host}). Ключ API — в настройках плагина, не в коде.

const BASE = {base};
const OPERATIONS = {table};

async function call(name, input = {{}}) {{
  const operation = OPERATIONS[name];
  let path = operation.path;
  for (const param of operation.pathParams) path = path.replace(`{{${{param}}}}`, encodeURIComponent(String(input[param] ?? "")));
  const url = new URL(BASE + path);
  for (const param of operation.query) if (input[param] !== undefined) url.searchParams.set(param, String(input[param]));
  const headers = {{ Accept: "application/json" }};
{auth}  const options = {{ method: operation.method, headers, timeoutMs: 1500 }};
  if (operation.body && input.body !== undefined) {{
    headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(input.body);
  }}
  const answer = await host.http.fetch(url.toString(), options);
  const text = await answer.text();
  if (!answer.ok) return {{ error: `HTTP ${{answer.status}}`, body: text.slice(0, 2000) }};
  try {{
    return JSON.parse(text);
  }} catch {{
    return text.slice(0, 8000);
  }}
}}

{exports}"#,
        base = json!(base),
        table = serde_json::to_string_pretty(&Value::Object(table)).unwrap_or_default(),
    );
    Ok(vec![
        ("manifest.json".into(), format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap_or_default())),
        ("backend.js".into(), backend),
        (
            "README.md".into(),
            format!("# {title}\n\nСделан из описания API ({host}). Что дальше: оставьте нужные инструменты, добавьте фразы (`contributes.intents`) и проверьте в мастерской.\n"),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest;

    const WEATHER: &str = r#"{
      "openapi": "3.0.0",
      "info": { "title": "Погода", "description": "Прогноз по городу" },
      "servers": [{ "url": "https://api.weather.example/v1" }],
      "components": { "securitySchemes": { "key": { "type": "apiKey", "in": "query", "name": "appid" } } },
      "paths": {
        "/forecast/{city}": {
          "get": {
            "operationId": "getForecast",
            "summary": "Прогноз погоды на несколько дней",
            "parameters": [
              { "name": "city", "in": "path", "required": true, "schema": { "type": "string" } },
              { "name": "days", "in": "query", "schema": { "type": "integer" }, "description": "Сколько дней" }
            ]
          }
        },
        "/alerts": { "post": { "summary": "Подписка на предупреждения", "requestBody": { "content": { "application/json": {} } } } }
      }
    }"#;

    #[test]
    fn makes_a_tool_per_operation_the_servers_host_its_only_right_and_the_key_a_setting() {
        let files = draft_from("weather-api", None, WEATHER).unwrap();
        let manifest_text = &files.iter().find(|(path, _)| path == "manifest.json").unwrap().1;
        let parsed = manifest::parse(manifest_text).unwrap();
        assert_eq!(parsed.title, "Погода");
        assert_eq!(parsed.hosts(), ["api.weather.example"]);
        // Operations in the order of their paths.
        let names: Vec<&str> = parsed.contributes.tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["post_alerts", "get_forecast"]);
        let forecast = &parsed.contributes.tools[1];
        assert!(forecast.read_only && !parsed.contributes.tools[0].read_only);
        assert_eq!(forecast.handler, "getForecast");
        assert_eq!(forecast.input["required"], json!(["city"]));
        assert_eq!(parsed.settings[0].key, "apiKey");
        let backend = &files.iter().find(|(path, _)| path == "backend.js").unwrap().1;
        assert!(backend.contains("export const getForecast = (input) => call(\"get_forecast\", input);"));
        assert!(backend.contains("url.searchParams.set(\"appid\", key)"));
        assert!(draft_from("x", None, r#"{"servers":[{"url":"http://api.example"}],"paths":{}}"#)
            .unwrap_err()
            .contains("https"));
        assert!(draft_from("x", None, "openapi: 3.0.0").unwrap_err().contains("JSON"));
    }
}
