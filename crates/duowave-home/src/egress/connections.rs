//! The connections an app gives the gateway (docs/plans/own-server.md, этап 2): the letter `system/network` →
//! `connections` `{items: [{id, name, country, url, state, current}]}` — each a local entry (HTTP and SOCKS on
//! `127.0.0.1`) of the app's program, its exit country and whether it answers. The app sends the letter when it
//! starts, on every change and every 30 s; a connection not heard of for 90 s does not answer.
//!
//! What was reported is kept (setting `egress.connections`, without the entries' addresses): after the gateway's
//! restart the rows that wait for a server keep waiting until the app speaks again, rather than go out directly for
//! the first seconds. A connection whose app is switched off or removed is dropped: the app off — everything directly.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A connection not heard of this long does not answer.
pub const STALE: Duration = Duration::from_secs(90);

/// The most connections one app may report.
const MAX_ITEMS: usize = 16;

/// One connection of an app.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    /// The app's id for it (`c1a2b3c`).
    pub id: String,
    /// Its name as the person gave it: «Мой VPN», «Дача».
    pub name: String,
    /// The exit country of its current server, ISO; None — not known yet.
    pub country: Option<String>,
    /// The current server's name, flag and all: «🇳🇱 Амстердам».
    pub current: Option<String>,
    /// Its local entry: `http://127.0.0.1:41234` (user and password may be in it). None until reported in this run.
    pub url: Option<url::Url>,
    /// Whether its current server answered the app's last check.
    pub state_up: bool,
    /// The app that reported it.
    pub app: String,
    /// When it was last reported in this run; None — known from before the gateway's restart only.
    pub seen: Option<Instant>,
}

impl Connection {
    /// It answers: reported up, recently, with an entry.
    pub fn up(&self) -> bool {
        self.state_up && self.url.is_some() && self.seen.is_some_and(|seen| seen.elapsed() < STALE)
    }

    #[cfg(test)]
    pub fn test(id: &str, country: &str, up: bool) -> Connection {
        Connection {
            id: id.into(),
            name: id.into(),
            country: Some(country.into()),
            current: None,
            url: Some(url::Url::parse("http://127.0.0.1:9").unwrap()),
            state_up: up,
            app: "own-server".into(),
            seen: Some(Instant::now()),
        }
    }
}

/// What is kept of a connection between the gateway's runs (`egress.connections`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub country: Option<String>,
    pub app: String,
}

/// The connections reported, by the app that reported them.
#[derive(Default)]
pub struct Connections {
    items: RwLock<Vec<Connection>>,
}

/// An ISO code in upper case, or None.
fn country_of(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (text.len() == 2 && text.chars().all(|c| c.is_ascii_alphabetic())).then(|| text.to_ascii_uppercase())
}

/// An entry on this machine only: `http://` or `socks5://` on a loopback address with a port.
fn entry_of(text: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(text).map_err(|_| crate::tr!("адрес входа «{text}» не читается", text = text))?;
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    if !matches!(url.scheme(), "http" | "socks5" | "socks5h") || !local || url.port().is_none() {
        return Err(crate::tr!("вход подключения — только http:// на 127.0.0.1 с портом"));
    }
    Ok(url)
}

impl Connections {
    /// Starts from what was kept: the connections are known, none answers until its app speaks.
    pub fn from_kept(kept: Vec<Kept>) -> Connections {
        let items = kept
            .into_iter()
            .map(|kept| Connection {
                id: kept.id,
                name: kept.name,
                country: kept.country,
                current: None,
                url: None,
                state_up: false,
                app: kept.app,
                seen: None,
            })
            .collect();
        Connections { items: RwLock::new(items) }
    }

    /// An app's letter: its connections now, all of them — the ones it no longer names are gone. Returns whether
    /// anything a decision reads changed (a connection came or went, its country, its answering).
    pub fn report(&self, app: &str, input: &Value) -> Result<bool, String> {
        let items = input.get("items").and_then(Value::as_array).ok_or_else(|| crate::tr!("нет списка items"))?;
        if items.len() > MAX_ITEMS {
            return Err(crate::tr!("подключений больше {max}", max = MAX_ITEMS));
        }
        let now = Instant::now();
        let mut fresh = Vec::new();
        for item in items {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(crate::tr!("у подключения нет id из латинских букв и цифр"));
            }
            if fresh.iter().any(|one: &Connection| one.id == id) {
                return Err(crate::tr!("подключение {id} названо дважды", id = id));
            }
            let url = entry_of(item.get("url").and_then(Value::as_str).unwrap_or_default())?;
            let text = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(|text| text.trim().chars().take(80).collect::<String>())
                    .filter(|text| !text.is_empty())
            };
            fresh.push(Connection {
                id: id.to_string(),
                name: text("name").unwrap_or_else(|| id.to_string()),
                country: country_of(item.get("country")),
                current: text("current"),
                url: Some(url),
                state_up: item.get("state").and_then(Value::as_str) == Some("up"),
                app: app.to_string(),
                seen: Some(now),
            });
        }
        let mut all = self.items.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Another app's connection by the same id: the newer report wins it.
        let before: Vec<(String, Option<String>, bool, String)> =
            all.iter().map(|one| (one.id.clone(), one.country.clone(), one.up(), one.app.clone())).collect();
        all.retain(|one| one.app != app && !fresh.iter().any(|new| new.id == one.id));
        all.extend(fresh);
        all.sort_by(|a, b| (a.app.as_str(), a.id.as_str()).cmp(&(b.app.as_str(), b.id.as_str())));
        let after: Vec<(String, Option<String>, bool, String)> =
            all.iter().map(|one| (one.id.clone(), one.country.clone(), one.up(), one.app.clone())).collect();
        Ok(before != after)
    }

    /// The connections of apps that are still switched on; the others are dropped. Whether any was dropped.
    pub fn keep_apps(&self, on: impl Fn(&str) -> bool) -> bool {
        let mut all = self.items.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = all.len();
        all.retain(|one| on(&one.app));
        all.len() != count
    }

    pub fn list(&self) -> Vec<Connection> {
        self.items.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// What to keep between runs.
    pub fn kept(&self) -> Vec<Kept> {
        self.list()
            .into_iter()
            .map(|one| Kept { id: one.id, name: one.name, country: one.country, app: one.app })
            .collect()
    }

    /// Whether a connection's answering changed since the last look: the stale ones, for the periodic rebuild.
    pub fn answering(&self) -> BTreeMap<String, bool> {
        self.list().into_iter().map(|one| (one.id.clone(), one.up())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_apps_letter_is_all_its_connections_now() {
        let connections = Connections::default();
        let letter = json!({ "items": [
            { "id": "c1", "name": "Мой VPN", "country": "nl", "url": "http://u:p@127.0.0.1:41234", "state": "up", "current": "🇳🇱 Амстердам" },
            { "id": "c2", "name": "Дача", "country": null, "url": "http://127.0.0.1:41235", "state": "down" },
        ]});
        assert!(connections.report("own-server", &letter).unwrap());
        let list = connections.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].country.as_deref(), Some("NL"));
        assert!(list[0].up() && !list[1].up());
        assert!(!connections.report("own-server", &letter).unwrap(), "the same letter changes nothing");
        connections.report("own-server", &json!({ "items": [] })).unwrap();
        assert!(connections.list().is_empty(), "named no more — gone");
    }

    #[test]
    fn an_entry_is_local_only() {
        let connections = Connections::default();
        for url in ["http://10.0.0.5:8080", "http://example.com:80", "ftp://127.0.0.1:21", "http://127.0.0.1"] {
            let letter = json!({ "items": [{ "id": "c1", "url": url, "state": "up" }] });
            assert!(connections.report("own-server", &letter).is_err(), "{url}");
        }
        let bad_id = json!({ "items": [{ "id": "../x", "url": "http://127.0.0.1:1", "state": "up" }] });
        assert!(connections.report("own-server", &bad_id).is_err());
    }

    #[test]
    fn kept_connections_wait_until_their_app_speaks_and_go_with_it() {
        let connections = Connections::from_kept(vec![Kept {
            id: "c1".into(),
            name: "Мой VPN".into(),
            country: Some("NL".into()),
            app: "own-server".into(),
        }]);
        assert!(!connections.list()[0].up(), "known, not answering before the app speaks");
        assert!(connections.keep_apps(|app| app != "own-server"));
        assert!(connections.list().is_empty(), "the app off — its connections gone");
    }
}
