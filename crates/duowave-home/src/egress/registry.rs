//! Who goes to the internet (docs/plans/own-server.md, «Таблица собирается сама»): the table is written nowhere whole —
//! the gateway gathers it from what stands in the home, every time something changes. Each row is one who goes:
//! Alice, a provider of «Мозг», hearing or voice, an MCP server, an app (its `net:` hosts, or its program through the
//! gateway's entry), an address the person typed. A new app, provider or tool is a new row with «Авто» at once; one
//! removed — its row goes. The rule is on the one who goes, not on an address: an app's new version with a new `net:`
//! host is under the same rule.
//!
//! Never in the table: this machine, the home network, the computer and the stations — always directly
//! (`is_local`).

use serde::Serialize;

/// A section of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// Alice's talk with Yandex.
    Alice,
    /// The brain's models.
    Brain,
    /// Cloud hearing and voice.
    Voice,
    /// The brain's tools (MCP servers).
    Mcp,
    /// The apps.
    Apps,
    /// The addresses the person typed.
    Own,
    /// Everything else DuoWave sends: the catalog, updates, downloads.
    Other,
}

/// One who goes to the internet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Goer {
    /// The id its rules are kept under (`app_settings`): `system:alice`, `provider:<id>`, `voice:elevenlabs`,
    /// `hearing:<host>`, `mcp:<id>`, `host:<name>`, or an app's package id.
    pub id: String,
    pub name: String,
    pub section: Section,
    /// The hosts it goes to: `api.openai.com`, `*.example.com` (its subdomains).
    pub hosts: Vec<String>,
    /// It carries a key or a sign-in: it never goes out directly from a country the service refuses, and never «tries
    /// directly first».
    pub keyed: bool,
    /// An app's own program: it goes through the gateway's entry (`HTTPS_PROXY`), decided by its row whatever the host.
    pub via_entry: bool,
    /// The id the catalog's `reach.json` knows it by, if other than `id`: `provider:openai` for ChatGPT's sign-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach_id: Option<String>,
}

/// The id of everything else DuoWave sends: the row hosts that no one else names fall into.
pub const OTHER: &str = "system:other";
/// Alice's row.
pub const ALICE: &str = "system:alice";

/// This machine, the home network, a station: never through a server (utils/net.rs, which every request asks).
pub use crate::utils::net::is_local;

/// A URL's host, small letters and no trailing dot: the base floor's (utils/http.rs).
pub use crate::utils::http::host_of;

/// Whether a host is one of a pattern: the same name, or a subdomain of `*.name`.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(base) => host.len() > base.len() && host.ends_with(base) && host[..host.len() - base.len()].ends_with('.'),
        None => pattern == host,
    }
}

/// What the table is gathered from, read from the gateway each time.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    /// Alice's upstream (`YANDEX_UNIPROXY_URL`).
    pub alice_url: Option<String>,
    /// «Мозг»: `(id, title, preset, base_url)` of each provider; `env` — the model of .env.
    pub providers: Vec<(String, String, String, Option<String>)>,
    /// Cloud hearing and voice: `(id, title, url)` — only those not at home.
    pub voices: Vec<(String, String, String)>,
    /// MCP servers: `(id, title, url)` of those that go over the network.
    pub mcp: Vec<(String, String, Option<String>)>,
    /// Apps: `(id, title, net hosts, own program)`; the app that gives the connections is not among them.
    pub apps: Vec<(String, String, Vec<String>, bool)>,
    /// The addresses the person typed.
    pub custom: Vec<String>,
}

/// A preset's address when the provider has none of its own.
fn provider_hosts(preset: &str, base_url: Option<&str>) -> Vec<String> {
    let mut hosts: Vec<String> = base_url
        .and_then(host_of)
        .or_else(|| {
            crate::dialog::brain::presets::preset_of(preset).and_then(|preset| preset.base_url).and_then(host_of)
        })
        .into_iter()
        .collect();
    // ChatGPT's sign-in: its plan answers at chatgpt.com, the sign-in at auth.openai.com.
    if preset == "chatgpt" {
        hosts.extend(["chatgpt.com".to_string(), "auth.openai.com".to_string()]);
    }
    hosts.retain(|host| !is_local(host));
    hosts.dedup();
    hosts
}

/// The table's rows from what stands in the home, in the table's order: Alice, «Мозг», hearing and voice, MCP, apps,
/// one's own addresses, everything else. A row with nothing on the internet (a model at home) is not in it.
pub fn gather(sources: &Sources) -> Vec<Goer> {
    let mut rows = Vec::new();
    if let Some(host) = sources.alice_url.as_deref().and_then(host_of).filter(|host| !is_local(host)) {
        rows.push(Goer {
            id: ALICE.into(),
            name: crate::tr!("Алиса — разговор с Яндексом"),
            section: Section::Alice,
            hosts: vec![host],
            keyed: false,
            via_entry: false,
            reach_id: None,
        });
    }
    for (id, title, preset, base_url) in &sources.providers {
        let hosts = provider_hosts(preset, base_url.as_deref());
        if hosts.is_empty() {
            continue;
        }
        // The catalog knows a provider by its preset: one's ChatGPT sign-in and one's OpenAI key go to one service.
        let reach = match preset.as_str() {
            "chatgpt" => Some("provider:openai".to_string()),
            "" | "custom-openai" | "custom-anthropic" => None,
            preset if preset != id => Some(format!("provider:{preset}")),
            _ => None,
        };
        rows.push(Goer {
            id: format!("provider:{id}"),
            name: title.clone(),
            section: Section::Brain,
            hosts,
            keyed: true,
            via_entry: false,
            reach_id: reach,
        });
    }
    for (id, title, url) in &sources.voices {
        let Some(host) = host_of(url).filter(|host| !is_local(host)) else { continue };
        rows.push(Goer {
            id: id.clone(),
            name: title.clone(),
            section: Section::Voice,
            hosts: vec![host],
            keyed: true,
            via_entry: false,
            reach_id: None,
        });
    }
    for (id, title, url) in &sources.mcp {
        let Some(host) = url.as_deref().and_then(host_of).filter(|host| !is_local(host)) else { continue };
        rows.push(Goer {
            id: format!("mcp:{id}"),
            name: title.clone(),
            section: Section::Mcp,
            hosts: vec![host],
            keyed: true,
            via_entry: false,
            reach_id: None,
        });
    }
    for (id, title, hosts, program) in &sources.apps {
        let hosts: Vec<String> =
            hosts.iter().filter(|host| !is_local(host.trim_start_matches("*."))).cloned().collect();
        if hosts.is_empty() && !program {
            continue;
        }
        rows.push(Goer {
            id: id.clone(),
            name: title.clone(),
            section: Section::Apps,
            hosts,
            keyed: false,
            via_entry: *program,
            reach_id: None,
        });
    }
    for host in &sources.custom {
        rows.push(Goer {
            id: format!("host:{host}"),
            name: host.clone(),
            section: Section::Own,
            hosts: vec![host.clone()],
            keyed: false,
            via_entry: false,
            reach_id: None,
        });
    }
    rows.push(Goer {
        id: OTHER.into(),
        name: crate::tr!("Остальное: каталог, обновления, загрузки"),
        section: Section::Other,
        hosts: Vec::new(),
        keyed: false,
        via_entry: false,
        reach_id: None,
    });
    rows
}

/// Which row a host belongs to when several name it: a row with a key first — its rule must hold —, then the
/// person's own address, Alice, the tools, the apps.
fn rank(goer: &Goer) -> u8 {
    match goer.section {
        Section::Brain | Section::Voice => 0,
        Section::Own => 1,
        Section::Alice => 2,
        Section::Mcp => 3,
        Section::Apps => 4,
        Section::Other => 5,
    }
}

/// The row of a host: an exact name first, then a `*.` pattern; within each, by `rank`. None — everything else.
pub fn row_of<'a>(rows: &'a [Goer], host: &str) -> Option<&'a Goer> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let exact = rows.iter().filter(|row| row.hosts.contains(&host)).min_by_key(|row| rank(row));
    exact.or_else(|| {
        rows.iter()
            .filter(|row| row.hosts.iter().any(|pattern| pattern.starts_with("*.") && host_matches(pattern, &host)))
            .min_by_key(|row| rank(row))
    })
}

/// An address the person types: a host name (`example.com`, `*.example.com`), lower case; the scheme and path of a
/// pasted URL are dropped. Not a host on the internet — None.
pub fn custom_host(text: &str) -> Option<String> {
    let text = text.trim().to_ascii_lowercase();
    let host = if text.contains("://") { host_of(&text)? } else { text.split(['/', ':']).next()?.to_string() };
    let bare = host.strip_prefix("*.").unwrap_or(&host);
    let labels: Vec<&str> = bare.split('.').collect();
    let valid = labels.len() >= 2
        && host.len() <= 253
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
        });
    (valid && !is_local(bare)).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Sources {
        Sources {
            alice_url: Some("wss://uniproxy.alice.yandex.net/uni.ws".into()),
            providers: vec![
                ("openai".into(), "OpenAI".into(), "openai".into(), None),
                ("mine".into(), "ChatGPT".into(), "chatgpt".into(), None),
                ("ollama".into(), "Ollama".into(), "ollama".into(), Some("http://localhost:11434/v1".into())),
            ],
            voices: vec![("voice:elevenlabs".into(), "ElevenLabs".into(), "https://api.elevenlabs.io".into())],
            mcp: vec![("exa".into(), "Exa".into(), Some("https://mcp.exa.ai/mcp".into()))],
            apps: vec![
                ("radio".into(), "Радио".into(), vec!["*.api.radio-browser.info".into()], false),
                ("airplay".into(), "AirPlay".into(), vec![], true),
                ("notes".into(), "Заметки".into(), vec![], false),
            ],
            custom: vec!["example.org".into()],
        }
    }

    #[test]
    fn the_table_gathers_itself_from_the_home() {
        let rows = gather(&sources());
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "system:alice",
                "provider:openai",
                "provider:mine",
                "voice:elevenlabs",
                "mcp:exa",
                "radio",
                "airplay",
                "host:example.org",
                "system:other"
            ],
            "a model at home and an app with no internet are not rows"
        );
        let mine = rows.iter().find(|row| row.id == "provider:mine").unwrap();
        assert_eq!(mine.reach_id.as_deref(), Some("provider:openai"));
        assert!(mine.hosts.contains(&"chatgpt.com".to_string()) && mine.keyed);
        assert!(rows.iter().find(|row| row.id == "airplay").unwrap().via_entry);
    }

    #[test]
    fn a_new_app_is_a_new_row_and_its_new_host_is_under_its_rule() {
        let mut home = sources();
        let before = gather(&home);
        home.apps.push(("maps".into(), "Карты".into(), vec!["tiles.example.com".into()], false));
        let after = gather(&home);
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(row_of(&after, "tiles.example.com").map(|row| row.id.as_str()), Some("maps"));
        // Its next version goes to a new host too: the same row, so the same rule.
        home.apps.last_mut().unwrap().2.push("*.cdn.example.net".into());
        assert_eq!(row_of(&gather(&home), "a.cdn.example.net").map(|row| row.id.as_str()), Some("maps"));
        home.apps.pop();
        assert_eq!(row_of(&gather(&home), "tiles.example.com"), None, "removed — its row went");
    }

    #[test]
    fn a_host_with_a_key_belongs_to_its_provider() {
        let mut home = sources();
        home.apps.push(("chat".into(), "Чат".into(), vec!["api.openai.com".into()], false));
        assert_eq!(row_of(&gather(&home), "api.openai.com").map(|row| row.id.as_str()), Some("provider:openai"));
    }

    #[test]
    fn local_addresses_are_never_in_the_table() {
        for host in
            ["127.0.0.1", "localhost", "192.168.1.20", "10.1.2.3", "station.local", "[::1]", "fe80::1", "duowave"]
        {
            assert!(is_local(host), "{host}");
        }
        for host in ["api.openai.com", "8.8.8.8", "uniproxy.alice.yandex.net"] {
            assert!(!is_local(host), "{host}");
        }
        assert!(host_matches("*.example.com", "a.example.com") && !host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "badexample.com"));
        assert_eq!(custom_host("https://Maps.Example.com/tiles?x=1").as_deref(), Some("maps.example.com"));
        assert_eq!(custom_host("*.example.com").as_deref(), Some("*.example.com"));
        assert_eq!(custom_host("192.168.1.1"), None);
        assert_eq!(custom_host("no-dot"), None);
    }
}
