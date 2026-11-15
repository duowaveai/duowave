//! Where services can be reached from (docs/plans/own-server.md, «Данные «где что работает» — в каталоге»): the
//! catalog's `reach.json`, signed as its feed and kept beside its copy (plugins/catalog.rs), and a feed entry's own
//! `reach` for an app. A copy of `reach.json` is built in, for a gateway that has not read the catalog yet; the
//! catalog's, once there, is newer. No words in it: ISO country codes and ids.
//!
//! ```json
//! { "format": 1, "updatedAt": "2026-11-16",
//!   "services": { "provider:openai": { "blocked": ["RU", "CN"], "hosts": ["api.openai.com"] },
//!                 "system:alice": { "only": ["RU"] } } }
//! ```
//!
//! A row finds its data by its catalog id (`reach_id`), its own id, then by any of its hosts — an MCP server or an
//! address the person typed is known by where it goes.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::registry::{host_matches, Goer};
use super::rules::Reach;

/// The copy built in.
const BUILT_IN: &str = include_str!("reach.json");

#[derive(Debug, Clone, Default, Deserialize)]
struct Service {
    #[serde(default)]
    blocked: Vec<String>,
    #[serde(default)]
    only: Vec<String>,
    #[serde(default)]
    hosts: Vec<String>,
}

/// The data: services by id, apps by package id.
#[derive(Debug, Clone, Default)]
pub struct ReachData {
    services: BTreeMap<String, Service>,
    apps: BTreeMap<String, Reach>,
    /// The `updatedAt` of the file it came from.
    pub updated_at: Option<String>,
}

fn upper(list: Vec<String>) -> Vec<String> {
    list.into_iter()
        .map(|code| code.trim().to_ascii_uppercase())
        .filter(|code| code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()))
        .collect()
}

fn parse(text: &str) -> Option<(BTreeMap<String, Service>, Option<String>)> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("format").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let services: BTreeMap<String, Service> = serde_json::from_value(value.get("services")?.clone()).ok()?;
    let services = services
        .into_iter()
        .map(|(id, service)| {
            let hosts = service.hosts.into_iter().map(|host| host.trim().to_ascii_lowercase()).collect();
            (id, Service { blocked: upper(service.blocked), only: upper(service.only), hosts })
        })
        .collect();
    Some((services, value.get("updatedAt").and_then(Value::as_str).map(str::to_string)))
}

impl ReachData {
    /// The built-in copy, then the catalog's `reach.json` when it is newer, and the apps' `reach` from the catalog's
    /// feed copy — both in the catalog's folder.
    pub fn load(catalog_dir: &Path) -> ReachData {
        let (mut services, mut updated_at) = parse(BUILT_IN).unwrap_or_default();
        if let Some((theirs, at)) =
            std::fs::read_to_string(catalog_dir.join("reach.json")).ok().as_deref().and_then(parse)
        {
            if at >= updated_at {
                services = theirs;
                updated_at = at;
            }
        }
        let mut apps = BTreeMap::new();
        if let Some(feed) = std::fs::read_to_string(catalog_dir.join("catalog.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            for item in feed.get("packages").and_then(Value::as_array).into_iter().flatten() {
                let (Some(id), Some(reach)) = (item.get("id").and_then(Value::as_str), item.get("reach")) else {
                    continue;
                };
                if let Ok(service) = serde_json::from_value::<Service>(reach.clone()) {
                    let reach = Reach { blocked: upper(service.blocked), only: upper(service.only) };
                    if !reach.is_empty() {
                        apps.insert(id.to_string(), reach);
                    }
                }
            }
        }
        ReachData { services, apps, updated_at }
    }

    #[cfg(test)]
    pub fn from_text(text: &str) -> ReachData {
        let (services, updated_at) = parse(text).unwrap_or_default();
        ReachData { services, apps: BTreeMap::new(), updated_at }
    }

    /// The data of a row: by its catalog id, its id, its app entry, then by its hosts.
    pub fn of(&self, goer: &Goer) -> Option<Reach> {
        let as_reach = |service: &Service| Reach { blocked: service.blocked.clone(), only: service.only.clone() };
        if let Some(service) = goer.reach_id.as_deref().and_then(|id| self.services.get(id)) {
            return Some(as_reach(service));
        }
        if let Some(service) = self.services.get(&goer.id) {
            return Some(as_reach(service));
        }
        if let Some(reach) = self.apps.get(&goer.id) {
            return Some(reach.clone());
        }
        self.services
            .values()
            .find(|service| {
                goer.hosts.iter().any(|host| {
                    let bare = host.trim_start_matches("*.");
                    service.hosts.iter().any(|known| known == bare || host_matches(known, bare))
                })
            })
            .map(as_reach)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::registry::Section;

    fn goer(id: &str, hosts: &[&str]) -> Goer {
        Goer {
            id: id.into(),
            name: id.into(),
            section: Section::Mcp,
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
            keyed: true,
            via_entry: false,
            reach_id: None,
        }
    }

    #[test]
    fn the_built_in_copy_reads_and_finds_by_id_and_host() {
        let data = ReachData::from_text(BUILT_IN);
        assert!(data.updated_at.is_some());
        let openai = data.of(&goer("provider:openai", &[])).expect("OpenAI is in the built-in copy");
        assert!(!openai.works_from("RU"));
        let alice = data.of(&goer("system:alice", &[])).expect("Alice too");
        assert!(alice.works_from("RU") && !alice.works_from("DE"));
        let by_host = data.of(&goer("mcp:7f3a", &["api.openai.com"])).expect("an MCP server by where it goes");
        assert_eq!(by_host, openai);
        assert_eq!(data.of(&goer("mcp:x", &["example.org"])), None);
    }

    #[test]
    fn a_newer_catalog_copy_wins_and_apps_bring_their_own() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("reach.json"),
            r#"{"format":1,"updatedAt":"2999-01-01","services":{"provider:openai":{"blocked":["de"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("catalog.json"),
            r#"{"format":1,"packages":[{"id":"maps","version":"1.0.0","reach":{"blocked":["RU"]}}]}"#,
        )
        .unwrap();
        let data = ReachData::load(dir.path());
        assert_eq!(data.updated_at.as_deref(), Some("2999-01-01"));
        assert!(!data.of(&goer("provider:openai", &[])).unwrap().works_from("DE"));
        assert!(!data.of(&goer("maps", &[])).unwrap().works_from("RU"));
    }
}
