//! Small per-device state that must outlive a restart (our timers, alarms, reminders, the player): read from the
//! database once when the store opens, then served from memory; every change is written through at once, the
//! device's list whole. In memory only when the database is (STATE_DIR set empty).

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use rusqlite::params;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::db::Db;
use crate::utils::time::now_iso;

pub struct StateStore<T> {
    db: Arc<Db>,
    kind: String,
    data: RwLock<BTreeMap<String, Vec<T>>>,
    _items: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned + Clone + Send + Sync> StateStore<T> {
    /// The store of a kind (`timers`, `alarms`, …), loaded now. Records that no longer read are left out (logged).
    pub fn open(db: Arc<Db>, kind: &str) -> StateStore<T> {
        let rows: Vec<(String, String)> = db
            .with(|c| {
                let mut statement = c.prepare("SELECT device, items FROM state WHERE kind = ?1")?;
                let rows = statement.query_map(params![kind], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .unwrap_or_else(|error| {
                tracing::warn!("{kind}: not loaded: {error}");
                Vec::new()
            });
        let mut data = BTreeMap::new();
        for (device, items) in rows {
            match serde_json::from_str::<Vec<serde_json::Value>>(&items) {
                Ok(values) => {
                    let parsed: Vec<T> = values
                        .into_iter()
                        .filter_map(|value| match serde_json::from_value(value) {
                            Ok(item) => Some(item),
                            Err(error) => {
                                tracing::warn!("{kind} of {device}: a record left out: {error}");
                                None
                            }
                        })
                        .collect();
                    if !parsed.is_empty() {
                        data.insert(device, parsed);
                    }
                }
                Err(error) => tracing::warn!("{kind} of {device}: not read: {error}"),
            }
        }
        StateStore { db, kind: kind.to_string(), data: RwLock::new(data), _items: PhantomData }
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The device's records (a copy).
    pub fn get(&self, device: &str) -> Vec<T> {
        self.data.read().unwrap().get(device).cloned().unwrap_or_default()
    }

    pub fn devices(&self) -> Vec<String> {
        self.data.read().unwrap().keys().cloned().collect()
    }

    /// The device's records, whole; an empty list removes them.
    pub fn set(&self, device: &str, items: Vec<T>) {
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        {
            let mut data = self.data.write().unwrap();
            if items.is_empty() {
                data.remove(device);
            } else {
                data.insert(device.to_string(), items);
            }
        }
        let kind = self.kind.clone();
        let saved = self.db.with(|c| {
            if json == "[]" {
                c.execute("DELETE FROM state WHERE kind = ?1 AND device = ?2", params![kind, device])?;
            } else {
                c.execute(
                    "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
                    params![kind, device, json, now_iso()],
                )?;
            }
            Ok(())
        });
        if let Err(error) = saved {
            tracing::warn!("{} of {device} not saved: {error}", self.kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Timer {
        id: String,
        ms: u64,
    }

    #[test]
    fn keeps_records_across_a_reopen_and_removes_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(dir.path())).unwrap());
        let store: StateStore<Timer> = StateStore::open(db.clone(), "timers");
        store.set("kitchen", vec![Timer { id: "t1".into(), ms: 5000 }]);
        store.set("hall", vec![Timer { id: "t2".into(), ms: 1 }]);
        store.set("hall", vec![]);
        drop(store);
        let again: StateStore<Timer> = StateStore::open(db, "timers");
        assert_eq!(again.get("kitchen"), vec![Timer { id: "t1".into(), ms: 5000 }]);
        assert_eq!(again.devices(), vec!["kitchen"]);
        assert!(again.get("hall").is_empty());
    }

    #[test]
    fn leaves_out_a_record_that_no_longer_reads() {
        let db = Arc::new(Db::memory());
        db.with(|c| {
            c.execute(
                "INSERT INTO state (kind, device, items, updated_at) VALUES ('timers', 'k', '[{\"id\":\"a\",\"ms\":1},{\"id\":5}]', 'now')",
                [],
            )
        })
        .unwrap();
        let store: StateStore<Timer> = StateStore::open(db, "timers");
        assert_eq!(store.get("k"), vec![Timer { id: "a".into(), ms: 1 }]);
    }
}
