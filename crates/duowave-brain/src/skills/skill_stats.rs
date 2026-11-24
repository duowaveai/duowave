//! "Not yet" and who served what: counters per capability for /health, the latest "not yet" requests for
//! GET /debug/unsupported. This journal is the queue of what to build next.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::capabilities::CapabilityName;
use crate::utils::time::now_iso;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedEntry {
    pub at: String,
    pub device: String,
    pub capability: CapabilityName,
    /// The request's text, or the callback's name.
    pub text: String,
    /// The capability's route as set: `own,yandex`, `off`.
    pub route: String,
    pub reason: String,
}

/// What a session tells about a request nobody could serve; the time is added.
#[derive(Debug, Clone)]
pub struct Unsupported {
    pub device: String,
    pub capability: CapabilityName,
    pub text: String,
    pub route: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Executor {
    Own,
    Yandex,
    Unsupported,
}

/// Requests per executor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Served {
    pub own: u64,
    pub yandex: u64,
    pub unsupported: u64,
}

const RECENT: usize = 200;

#[derive(Default)]
struct Inner {
    log: VecDeque<UnsupportedEntry>,
    unsupported: BTreeMap<CapabilityName, u64>,
    served: BTreeMap<CapabilityName, Served>,
}

#[derive(Default)]
pub struct SkillStats {
    inner: Mutex<Inner>,
}

impl SkillStats {
    pub fn new() -> SkillStats {
        SkillStats::default()
    }

    pub fn unsupported(&self, entry: Unsupported) -> UnsupportedEntry {
        let record = UnsupportedEntry {
            at: now_iso(),
            device: entry.device,
            capability: entry.capability,
            text: entry.text,
            route: entry.route,
            reason: entry.reason,
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.log.push_back(record.clone());
            if inner.log.len() > RECENT {
                inner.log.pop_front();
            }
            *inner.unsupported.entry(record.capability).or_default() += 1;
        }
        self.count(record.capability, Executor::Unsupported);
        record
    }

    pub fn count(&self, capability: CapabilityName, executor: Executor) {
        let mut inner = self.inner.lock().unwrap();
        let counts = inner.served.entry(capability).or_default();
        match executor {
            Executor::Own => counts.own += 1,
            Executor::Yandex => counts.yandex += 1,
            Executor::Unsupported => counts.unsupported += 1,
        }
    }

    /// The latest first, at most `limit` (at least one).
    pub fn recent_unsupported(&self, limit: usize) -> Vec<UnsupportedEntry> {
        let inner = self.inner.lock().unwrap();
        inner.log.iter().rev().take(limit.max(1)).cloned().collect()
    }

    pub fn unsupported_by_capability(&self) -> BTreeMap<CapabilityName, u64> {
        self.inner.lock().unwrap().unsupported.clone()
    }

    pub fn served_by_capability(&self) -> BTreeMap<CapabilityName, Served> {
        self.inner.lock().unwrap().served.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> Unsupported {
        Unsupported {
            device: "kitchen".into(),
            capability: CapabilityName::Radio,
            text: text.into(),
            route: "off".into(),
            reason: "route off".into(),
        }
    }

    #[test]
    fn counts_and_keeps_the_latest_first() {
        let stats = SkillStats::new();
        for i in 0..205 {
            stats.unsupported(entry(&format!("радио {i}")));
        }
        stats.count(CapabilityName::Radio, Executor::Yandex);
        let recent = stats.recent_unsupported(2);
        assert_eq!(recent.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["радио 204", "радио 203"]);
        assert_eq!(stats.recent_unsupported(1000).len(), RECENT);
        assert_eq!(stats.unsupported_by_capability()[&CapabilityName::Radio], 205);
        assert_eq!(
            stats.served_by_capability()[&CapabilityName::Radio],
            Served { own: 0, yandex: 1, unsupported: 205 }
        );
        assert_eq!(
            serde_json::to_value(stats.served_by_capability()).unwrap(),
            serde_json::json!({"radio": {"own": 0, "yandex": 1, "unsupported": 205}})
        );
    }
}
