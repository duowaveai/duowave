//! The server configs people already have: Claude Desktop and Cursor write `{"mcpServers": {name: {...}}}`, VS Code
//! `{"servers": {...}}` (or `{"mcp": {"servers": ...}}` in its settings), a single server may come bare. Each entry is
//! a program (`command`, `args`, `env`) or an address (`url`/`serverUrl`, `headers`). Values of variables and
//! headers are secrets: the gateway seals them; nothing else of the JSON is kept.

use std::collections::HashMap;
use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

use super::mcp_presets::TransportKind;

/// JSON as it was written: objects keep their keys in order (serde_json's maps sort them), so pasted servers are
/// added, and their variables listed, in the order the owner wrote them.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A value that is not null (JavaScript's `??` falls through null and undefined alike).
    pub(crate) fn present(value: Option<&Json>) -> Option<&Json> {
        value.filter(|value| !matches!(value, Json::Null))
    }

    /// A list of strings: every item must be one.
    pub fn strings(&self) -> Option<Vec<String>> {
        self.as_array()?.iter().map(|item| item.as_str().map(str::to_string)).collect()
    }

    /// An object of plain values as strings (numbers and booleans as JavaScript's String() writes them); None when
    /// it is not one.
    pub fn record(&self) -> Option<Vec<(String, String)>> {
        self.as_object()?
            .iter()
            .map(|(key, item)| {
                let text = match item {
                    Json::String(text) => text.clone(),
                    Json::Number(number) => js_number(number.as_f64().unwrap_or_default()),
                    Json::Bool(flag) => flag.to_string(),
                    _ => return None,
                };
                Some((key.clone(), text))
            })
            .collect()
    }
}

/// A number as JavaScript's String() writes it (Number::toString): JSON.parse reads every number as a double, which is
/// written with the fewest digits that read back the same, with no fraction when it has none (1.0 is "1", 1e3 is
/// "1000", -0 is "0"), and with an exponent from 21 digits on or before six zeros (1e21 is "1e+21", 1e-7 "1e-7").
fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.is_nan() {
        return "NaN".into();
    }
    if value < 0.0 {
        return format!("-{}", js_number(-value));
    }
    if value.is_infinite() {
        return "Infinity".into();
    }
    // Rust's shortest exponent form gives the digits and where the point is: "1.2345e3".
    let written = format!("{value:e}");
    let Some((mantissa, exponent)) = written.split_once('e') else { return value.to_string() };
    let Ok(exponent) = exponent.parse::<i32>() else { return value.to_string() };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    // The value is 0.<digits> × 10^n.
    let n = exponent + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(-n as usize))
    } else {
        let fraction = if k > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        let sign = if n > 0 { '+' } else { '-' };
        format!("{}{fraction}e{sign}{}", &digits[..1], (n - 1).abs())
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Json, D::Error> {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
                Ok(Json::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
                Ok(Json::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
                Ok(Json::Number(value.into()))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(value).map(Json::Number).unwrap_or(Json::Null))
            }

            fn visit_str<E>(self, value: &str) -> Result<Json, E> {
                Ok(Json::String(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Json, E> {
                Ok(Json::String(value))
            }

            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Json, D::Error> {
                Json::deserialize(deserializer)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut entries: Vec<(String, Json)> = Vec::new();
                // Where each key stands in `entries`: one lookup per key. Every loose field of the MCP bodies is
                // read here before the route checks who is asking, so an object of many keys must read in linear
                // time (a scan of the entries per key took a 2 MB body of keys tens of seconds).
                let mut places: HashMap<String, usize> = HashMap::new();
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    // A repeated key keeps its first place and its last value, as JSON.parse does.
                    match places.get(&key) {
                        Some(&place) => entries[place].1 = value,
                        None => {
                            places.insert(key.clone(), entries.len());
                            entries.push((key, value));
                        }
                    }
                }
                Ok(Json::Object(entries))
            }
        }

        deserializer.deserialize_any(JsonVisitor)
    }
}

/// For an optional body field that may be null: absent is None, null is `Some(Json::Null)`.
pub fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Json>, D::Error> {
    Json::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedServer {
    pub name: String,
    pub transport: TransportKind,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<Vec<(String, String)>>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
}

impl ImportedServer {
    fn address(name: &str, url: &str, headers: Option<Vec<(String, String)>>) -> ImportedServer {
        ImportedServer {
            name: name.to_string(),
            transport: TransportKind::Http,
            command: None,
            args: None,
            env: None,
            cwd: None,
            url: Some(url.to_string()),
            headers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportResult {
    pub servers: Vec<ImportedServer>,
    /// What could not be read, for people.
    pub problems: Vec<String>,
}

fn http_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
}

fn entry(name: &str, value: &Json) -> Result<ImportedServer, String> {
    if value.as_object().is_none() {
        return Err(crate::tr!("«{name}»: это не описание сервера.", name = name));
    }
    if value.get("disabled").and_then(Json::as_bool) == Some(true) {
        return Err(crate::tr!("«{name}» выключен в файле — пропущен.", name = name));
    }
    let url = match value.get("url") {
        Some(Json::String(url)) => Some(url.as_str()),
        _ => value.get("serverUrl").and_then(Json::as_str),
    };
    if let Some(url) = url.filter(|url| !url.is_empty()) {
        if !http_url(url) {
            return Err(crate::tr!("«{name}»: адрес {url} не http(s).", name = name, url = url));
        }
        return Ok(ImportedServer::address(name, url, value.get("headers").and_then(Json::record)));
    }
    if let Some(command) = value.get("command").and_then(Json::as_str).map(str::trim).filter(|c| !c.is_empty()) {
        let args = match value.get("args") {
            None => Vec::new(),
            Some(args) => args.strings().ok_or_else(|| crate::tr!("«{name}»: args — не список строк.", name = name))?,
        };
        let env = match value.get("env") {
            None => None,
            Some(env) => Some(env.record().ok_or_else(|| crate::tr!("«{name}»: env — не набор строк.", name = name))?),
        };
        return Ok(ImportedServer {
            name: name.to_string(),
            transport: TransportKind::Stdio,
            command: Some(command.to_string()),
            args: Some(args),
            env,
            cwd: value.get("cwd").and_then(Json::as_str).map(str::to_string),
            url: None,
            headers: None,
        });
    }
    Err(crate::tr!("«{name}»: нет ни command, ни url.", name = name))
}

/// Servers from a pasted config; problems say what was skipped and why.
pub fn parse_import(text: &str) -> ImportResult {
    let Ok(data) = serde_json::from_str::<Json>(text) else {
        return ImportResult { servers: Vec::new(), problems: vec![crate::tr!("Это не JSON.")] };
    };
    if data.as_object().is_none() {
        return ImportResult { servers: Vec::new(), problems: vec![crate::tr!("Ожидался объект JSON.")] };
    }
    let nested = data.get("mcp").and_then(|mcp| mcp.get("servers"));
    let map = Json::present(data.get("mcpServers")).or_else(|| Json::present(data.get("servers"))).or(nested);
    let named: Vec<(String, Json)> = match map.and_then(Json::as_object) {
        Some(entries) => entries.to_vec(),
        None if ["command", "url", "serverUrl"].iter().any(|key| data.get(key).is_some()) => {
            let name = data.get("name").and_then(Json::as_str).unwrap_or("server").to_string();
            vec![(name, data.clone())]
        }
        None => Vec::new(),
    };
    if named.is_empty() {
        return ImportResult {
            servers: Vec::new(),
            problems: vec![crate::tr!(
                "Не нашли серверов: ждём mcpServers, servers или одно описание с command или url."
            )],
        };
    }
    let mut result = ImportResult::default();
    for (name, value) in &named {
        match entry(name, value) {
            Ok(server) => result.servers.push(server),
            Err(problem) => result.problems.push(problem),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
    }

    #[test]
    fn reads_claude_desktop_vs_code_and_a_bare_server_and_says_what_it_skipped() {
        assert_eq!(
            parse_import(r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "pkg"], "env": {"TOKEN": 5}}}}"#),
            ImportResult {
                servers: vec![ImportedServer {
                    name: "fs".into(),
                    transport: TransportKind::Stdio,
                    command: Some("npx".into()),
                    args: Some(vec!["-y".into(), "pkg".into()]),
                    env: Some(pairs(&[("TOKEN", "5")])),
                    cwd: None,
                    url: None,
                    headers: None,
                }],
                problems: vec![],
            }
        );
        assert_eq!(
            parse_import(r#"{"mcp": {"servers": {"notion": {"type": "http", "url": "https://mcp.notion.com/mcp"}}}}"#)
                .servers,
            vec![ImportedServer::address("notion", "https://mcp.notion.com/mcp", None)]
        );
        assert_eq!(
            parse_import(r#"{"serverUrl": "https://example.org/mcp", "headers": {"X-Key": "k"}}"#).servers,
            vec![ImportedServer::address("server", "https://example.org/mcp", Some(pairs(&[("X-Key", "k")])))]
        );
        assert_eq!(
            parse_import(
                r#"{"servers": {"a": {"command": "x", "args": "y"}, "b": {"url": "ftp://x"}, "c": {"disabled": true, "command": "z"}}}"#
            )
            .problems,
            vec!["«a»: args — не список строк.", "«b»: адрес ftp://x не http(s).", "«c» выключен в файле — пропущен."]
        );
        assert_eq!(parse_import("not json").problems, vec!["Это не JSON."]);
        assert!(parse_import(r#"{"other": 1}"#).problems[0].starts_with("Не нашли серверов"));
    }

    #[test]
    fn keeps_the_order_the_servers_were_written_in() {
        let result = parse_import(
            r#"{"mcpServers": {"zeta": {"command": "z"}, "alpha": {"command": "a", "env": {"B": "1", "A": "2"}}}}"#,
        );
        assert_eq!(result.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["zeta", "alpha"]);
        assert_eq!(result.servers[1].env, Some(pairs(&[("B", "1"), ("A", "2")])));
    }

    #[test]
    fn reads_an_object_of_many_keys_in_linear_time_a_repeated_key_in_its_first_place_with_its_last_value() {
        let repeated: Json = serde_json::from_str(r#"{"a": 1, "b": 2, "a": 3}"#).unwrap();
        assert_eq!(
            repeated,
            Json::Object(vec![("a".into(), Json::Number(3.into())), ("b".into(), Json::Number(2.into()))])
        );
        // A body of 100 000 keys (1.1 MB): with a scan of the entries per key this took tens of seconds, before any
        // access check.
        let keys: Vec<String> = (0..100_000).map(|n| format!("\"{n:06}\":0")).collect();
        let body = format!("{{\"question\":{{{}}}}}", keys.join(","));
        let started = std::time::Instant::now();
        let parsed: Json = serde_json::from_str(&body).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(parsed.get("question").and_then(Json::as_object).map(<[_]>::len), Some(100_000));
        assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn writes_the_numbers_of_variables_and_headers_as_javascripts_string_does() {
        // (A decimal of 17 digits and more may still differ in its last digit: serde_json reads such a number to
        // within a unit of its last place, JSON.parse exactly.)
        let result = parse_import(
            r#"{"mcpServers": {"x": {"command": "npx", "env": {
                "RETRIES": 1.0, "LIMIT": 1e3, "ZERO": -0, "COUNT": 5, "BELOW": -3, "HALF": 0.5, "PI": 3.14159,
                "TINY": 0.0000015, "TINIER": 1e-7, "BIG": 1e21, "BIGGER": 1.5e300, "LONG": 100000000000000000000,
                "UNSAFE": 12345678901234567890, "ON": true
            }}}}"#,
        );
        assert_eq!(
            result.servers[0].env,
            Some(pairs(&[
                ("RETRIES", "1"),
                ("LIMIT", "1000"),
                ("ZERO", "0"),
                ("COUNT", "5"),
                ("BELOW", "-3"),
                ("HALF", "0.5"),
                ("PI", "3.14159"),
                ("TINY", "0.0000015"),
                ("TINIER", "1e-7"),
                ("BIG", "1e+21"),
                ("BIGGER", "1.5e+300"),
                ("LONG", "100000000000000000000"),
                ("UNSAFE", "12345678901234567000"),
                ("ON", "true"),
            ]))
        );
    }
}
