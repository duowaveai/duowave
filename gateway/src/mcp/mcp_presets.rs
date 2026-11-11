//! MCP servers the panel offers ready-made (the owner's choice of 06.11: Home Assistant, a web search, tasks and
//! notes). A preset knows how to start or reach the server, what the owner fills in, which tools are on at first
//! sight and whether its answers bring text from the internet. Versions are pinned: a package changes only when we
//! change it here. Programs are fetched by npx/uvx on their first start — the panel shows what and starts it on the
//! owner's button.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::skills::capabilities::CapabilityName;

/// How the gateway reaches a server: a program on this computer, or an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Stdio,
    Http,
}

impl TransportKind {
    pub fn parse(value: &str) -> Option<TransportKind> {
        match value {
            "stdio" => Some(TransportKind::Stdio),
            "http" => Some(TransportKind::Http),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Stdio => "stdio",
            TransportKind::Http => "http",
        }
    }
}

/// How a server by address is signed in to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    #[default]
    None,
    Token,
    Oauth,
}

impl AuthKind {
    pub fn parse(value: &str) -> Option<AuthKind> {
        match value {
            "none" => Some(AuthKind::None),
            "token" => Some(AuthKind::Token),
            "oauth" => Some(AuthKind::Oauth),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetField {
    /// For a program: the variable's name; for a server by address: `token`.
    pub name: &'static str,
    pub title: &'static str,
    pub secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<&'static str>,
    /// Where the owner gets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<&'static str>,
}

/// What the first start downloads, for the panel to say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    pub runner: &'static str,
    pub package: &'static str,
}

/// Tools on at first sight: all of them, the ones the server marks read-only, and these names.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enable {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub names: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
pub struct McpPreset {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<&'static str>,
    pub transport: TransportKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<&'static [&'static str]>,
    /// The address; `{HA_URL}` is the Home Assistant address the gateway already knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<&'static str>,
    pub auth: AuthKind,
    pub fields: &'static [PresetField],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<Install>,
    pub enable: Enable,
    /// Its answers bring text from the internet: after one, tools that change things are not offered in that answer.
    pub outside: bool,
    /// The capability it acts as: its route and its denials apply (Home Assistant is the smart home).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilityName>,
    /// A hint for the model, offered as the owner's note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
    pub order: u32,
}

const SEARCH_NOTE: &str = "Ищи, когда нужен свежий факт или новость. Называй источник словами, без ссылок.";

pub static MCP_PRESETS: &[McpPreset] = &[
    McpPreset {
        id: "homeassistant",
        title: "Home Assistant",
        description: "Свет, розетки, климат и сценарии через встроенный MCP-сервер Home Assistant: только устройства, открытые голосовым помощникам.",
        brand: Some("homeassistant"),
        transport: TransportKind::Http,
        command: None,
        args: None,
        url: Some("{HA_URL}/api/mcp/assist"),
        auth: AuthKind::Token,
        fields: &[PresetField {
            name: "token",
            title: "Долгоживущий токен Home Assistant",
            secret: true,
            placeholder: Some("eyJ…"),
            url: Some("{HA_URL}/profile/security"),
        }],
        install: None,
        enable: Enable { all: true, read_only: false, names: &[] },
        outside: false,
        capability: Some(CapabilityName::SmartHome),
        note: Some("Это наш дом. Устройство и комнату называй так, как их назвал человек."),
        order: 1,
    },
    McpPreset {
        id: "tavily",
        title: "Поиск Tavily",
        description: "Поиск в интернете для свежих ответов: новости, цены, факты. Нужен ключ Tavily.",
        brand: Some("tavily"),
        transport: TransportKind::Stdio,
        command: Some("npx"),
        args: Some(&["-y", "tavily-mcp@0.2.22"]),
        url: None,
        auth: AuthKind::None,
        fields: &[PresetField {
            name: "TAVILY_API_KEY",
            title: "Ключ Tavily",
            secret: true,
            placeholder: Some("tvly-…"),
            url: Some("https://app.tavily.com"),
        }],
        install: Some(Install { runner: "npx", package: "tavily-mcp@0.2.22" }),
        enable: Enable { all: false, read_only: false, names: &["tavily-search", "tavily-extract"] },
        outside: true,
        capability: None,
        note: Some(SEARCH_NOTE),
        order: 2,
    },
    McpPreset {
        id: "brave",
        title: "Поиск Brave",
        description: "Поиск в интернете Brave Search: страницы, новости, места рядом. Нужен ключ Brave Search API.",
        brand: Some("brave"),
        transport: TransportKind::Stdio,
        command: Some("npx"),
        args: Some(&["-y", "@brave/brave-search-mcp-server@2.1.4", "--transport", "stdio"]),
        url: None,
        auth: AuthKind::None,
        fields: &[PresetField {
            name: "BRAVE_API_KEY",
            title: "Ключ Brave Search API",
            secret: true,
            placeholder: Some("BSA…"),
            url: Some("https://brave.com/search/api/"),
        }],
        install: Some(Install { runner: "npx", package: "@brave/brave-search-mcp-server@2.1.4" }),
        enable: Enable {
            all: false,
            read_only: false,
            names: &["brave_web_search", "brave_news_search", "brave_local_search"],
        },
        outside: true,
        capability: None,
        note: Some(SEARCH_NOTE),
        order: 3,
    },
    McpPreset {
        id: "fetch",
        title: "Чтение страниц",
        description: "Открывает страницу по адресу и отдаёт её текст: к поиску, чтобы прочитать найденное.",
        brand: None,
        transport: TransportKind::Stdio,
        command: Some("uvx"),
        args: Some(&["--from", "mcp-server-fetch==2026.11.4", "mcp-server-fetch"]),
        url: None,
        auth: AuthKind::None,
        fields: &[],
        install: Some(Install { runner: "uvx", package: "mcp-server-fetch==2026.11.4" }),
        enable: Enable { all: false, read_only: false, names: &["fetch"] },
        outside: true,
        capability: None,
        note: None,
        order: 4,
    },
    McpPreset {
        id: "todoist",
        title: "Todoist",
        description: "Задачи и списки: что на сегодня, добавить покупку, отметить сделанное. Вход через ваш аккаунт Todoist.",
        brand: Some("todoist"),
        transport: TransportKind::Http,
        command: None,
        args: None,
        url: Some("https://ai.todoist.net/mcp"),
        auth: AuthKind::Oauth,
        fields: &[],
        install: None,
        enable: Enable { all: false, read_only: true, names: &[] },
        outside: false,
        capability: None,
        note: Some("Это наш общий список дел."),
        order: 5,
    },
    McpPreset {
        id: "notion",
        title: "Notion",
        description: "Заметки и страницы Notion: найти, прочитать, дописать. Вход через ваш аккаунт Notion.",
        brand: Some("notion"),
        transport: TransportKind::Http,
        command: None,
        args: None,
        url: Some("https://mcp.notion.com/mcp"),
        auth: AuthKind::Oauth,
        fields: &[],
        install: None,
        enable: Enable { all: false, read_only: true, names: &[] },
        outside: true,
        capability: None,
        note: None,
        order: 6,
    },
];

pub fn mcp_preset(id: Option<&str>) -> Option<&'static McpPreset> {
    let id = id?;
    MCP_PRESETS.iter().find(|preset| preset.id == id)
}

/// A preset's text with the addresses the gateway knows filled in; None while one is unknown.
pub fn filled(text: &str, ha_url: Option<&str>) -> Option<String> {
    if !text.contains("{HA_URL}") {
        return Some(text.to_string());
    }
    let ha = ha_url.filter(|url| !url.is_empty())?;
    Some(text.replace("{HA_URL}", ha.trim_end_matches('/')))
}

/// A preset as the panel shows it: the addresses filled in, and whether it can be added now (and why not).
pub fn preset_view(preset: &McpPreset, ha_url: Option<&str>) -> Value {
    let mut view = serde_json::to_value(preset).unwrap_or_else(|_| json!({}));
    let url = preset.url.map(|url| filled(url, ha_url));
    if let (Some(original), Some(url)) = (preset.url, &url) {
        view["url"] = json!(url.clone().unwrap_or_else(|| original.to_string()));
    }
    let fields: Vec<Value> = preset
        .fields
        .iter()
        .map(|field| {
            let mut entry = serde_json::to_value(field).unwrap_or_else(|_| json!({}));
            if let Some(url) = field.url {
                entry["url"] = json!(filled(url, ha_url).unwrap_or_else(|| url.to_string()));
            }
            entry
        })
        .collect();
    view["fields"] = Value::Array(fields);
    let unknown = matches!(url, Some(None));
    view["ready"] = json!(!unknown);
    if unknown {
        view["why"] = json!("Нужен адрес Home Assistant: в «Навыки» → «Подключения» или свой адрес.");
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_the_home_assistant_address_or_says_why_a_preset_waits() {
        assert_eq!(
            filled("{HA_URL}/api/mcp/assist", Some("http://ha.local:8123/")).as_deref(),
            Some("http://ha.local:8123/api/mcp/assist")
        );
        assert_eq!(filled("{HA_URL}/api/mcp/assist", None), None);
        assert_eq!(filled("https://mcp.notion.com/mcp", None).as_deref(), Some("https://mcp.notion.com/mcp"));
        let ha = mcp_preset(Some("homeassistant")).unwrap();
        let waiting = preset_view(ha, None);
        assert_eq!(waiting["ready"], json!(false));
        assert_eq!(waiting["url"], json!("{HA_URL}/api/mcp/assist"));
        assert!(waiting["why"].as_str().unwrap().contains("«Подключения»"));
        let ready = preset_view(ha, Some("http://ha.local:8123"));
        assert_eq!(ready["ready"], json!(true));
        assert_eq!(ready["url"], json!("http://ha.local:8123/api/mcp/assist"));
        assert_eq!(ready["fields"][0]["url"], json!("http://ha.local:8123/profile/security"));
        assert_eq!(ready["capability"], json!("smart_home"));
        assert_eq!(ready["enable"], json!({ "all": true }));
        assert!(ready.get("why").is_none());
        let tavily = preset_view(mcp_preset(Some("tavily")).unwrap(), None);
        assert_eq!(tavily["ready"], json!(true));
        assert_eq!(tavily["install"], json!({ "runner": "npx", "package": "tavily-mcp@0.2.22" }));
        assert!(tavily.get("url").is_none());
        assert!(mcp_preset(Some("nope")).is_none());
        assert!(mcp_preset(None).is_none());
    }
}
