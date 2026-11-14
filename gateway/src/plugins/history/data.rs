//! A plugin's data kept and brought back (docs/plans/package-history.md, «Возврат поставленного приложения»): its
//! `kv` (the `plugin_kv` rows), its jobs (plugins/schedule.rs) and its pictures (plugins/files.rs) — never its settings
//! and keys: they are the admin's, not the plugin's, and they stay out of the history.
//!
//! A snapshot leaves out what changes by itself — a row's time, a job's last run and the speech waiting for the
//! station — so the same data is the same object, and a day without changes keeps nothing new. Pictures are files of
//! the store: one that did not change takes no room.
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{Kind, Store};
use crate::plugins::files::PluginFiles;
use crate::plugins::schedule::Scheduler;
use crate::storage::Db;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvRow {
    pub key: String,
    /// The value as the store keeps it: JSON text.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    pub name: String,
    pub file: String,
    pub size: u64,
}

/// A plugin's data at a moment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kv: Vec<KvRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<Picture>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.kv.is_empty() && self.jobs.is_empty() && self.pictures.is_empty()
    }

    /// In words: «12 записей, 2 задачи, 3 картинки».
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.kv.is_empty() {
            parts.push(crate::tr!("записей: {kv}", kv = self.kv.len()));
        }
        if !self.jobs.is_empty() {
            parts.push(crate::tr!("задач: {jobs}", jobs = self.jobs.len()));
        }
        if !self.pictures.is_empty() {
            parts.push(crate::tr!("картинок: {pictures}", pictures = self.pictures.len()));
        }
        if parts.is_empty() {
            crate::tr!("пусто")
        } else {
            parts.join(", ")
        }
    }
}

/// Where a plugin's data lives.
pub struct DataParts {
    pub db: Arc<Db>,
    pub schedules: Option<Arc<Scheduler>>,
    pub files: Option<Arc<PluginFiles>>,
}

impl DataParts {
    /// The plugin's data now.
    pub fn snapshot(&self, store: &Store, plugin: &str) -> Result<Snapshot, String> {
        let kv = crate::plugins::host_api::kv_rows(&self.db, plugin)
            .map_err(|error| crate::tr!("хранилище не читается: {error}", error = error))?
            .into_iter()
            .map(|(key, value)| KvRow { key, value })
            .collect();
        let jobs = self.schedules.as_ref().map(|schedules| schedules.export(plugin)).unwrap_or_default();
        let mut pictures = Vec::new();
        if let Some(files) = &self.files {
            for listed in files.list(plugin) {
                let Some(name) = listed.get("name").and_then(Value::as_str) else { continue };
                let Some((bytes, _)) = files.read(plugin, name) else { continue };
                let file = store
                    .put(Kind::File, &bytes)
                    .map_err(|error| crate::tr!("картинка {name}: {error}", name = name, error = error))?;
                pictures.push(Picture { name: name.to_string(), file, size: bytes.len() as u64 });
            }
        }
        Ok(Snapshot { v: 1, kv, jobs, pictures })
    }

    /// The plugin's data now, kept in the store: its id. An empty one is kept too — «it had nothing then» is not
    /// «nobody looked», and a step back to it empties the data.
    pub fn capture(&self, store: &Store, plugin: &str) -> Result<Option<String>, String> {
        let snapshot = self.snapshot(store, plugin)?;
        let bytes = serde_json::to_vec(&snapshot).map_err(|error| error.to_string())?;
        store.put(Kind::Data, &bytes).map(Some).map_err(|error| error.to_string())
    }

    /// A kept snapshot, read.
    pub fn read(store: &Store, id: &str) -> Option<Snapshot> {
        serde_json::from_slice(&store.get_kind(id, Kind::Data)?).ok()
    }

    /// The plugin's data replaced by a kept snapshot (None — emptied): the pictures first, as they may not fit the
    /// station; then the store; then the jobs, a once already past left out.
    pub fn apply(&self, store: &Store, plugin: &str, id: Option<&str>) -> Result<(), String> {
        let snapshot = match id {
            Some(id) => {
                DataParts::read(store, id).ok_or_else(|| crate::tr!("этого снимка данных в истории уже нет"))?
            }
            None => Snapshot::default(),
        };
        if let Some(files) = &self.files {
            let mut pictures = Vec::new();
            for picture in &snapshot.pictures {
                let bytes = store
                    .get_kind(&picture.file, Kind::File)
                    .ok_or_else(|| crate::tr!("картинки {name} в истории уже нет", name = picture.name))?;
                pictures.push((picture.name.clone(), bytes));
            }
            files.replace(plugin, &pictures)?;
        }
        let rows: Vec<(String, String)> = snapshot.kv.iter().map(|row| (row.key.clone(), row.value.clone())).collect();
        crate::plugins::host_api::kv_replace(&self.db, plugin, &rows)
            .map_err(|error| crate::tr!("хранилище не записалось: {error}", error = error))?;
        if let Some(schedules) = &self.schedules {
            schedules.import(plugin, &snapshot.jobs)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_plugins_store_and_brings_it_back_the_same_data_the_same_object() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("history")).unwrap();
        let db = Arc::new(Db::memory());
        let parts = DataParts { db: db.clone(), schedules: None, files: None };
        let empty = parts.capture(&store, "shopping").unwrap().unwrap();
        assert_eq!(DataParts::read(&store, &empty).unwrap().summary(), "пусто");
        let rows = vec![("list".to_string(), "{\"items\":[\"молоко\"]}".to_string())];
        crate::plugins::host_api::kv_replace(&db, "shopping", &rows).unwrap();
        let first = parts.capture(&store, "shopping").unwrap().unwrap();
        assert_eq!(parts.capture(&store, "shopping").unwrap().as_deref(), Some(first.as_str()));
        crate::plugins::host_api::kv_replace(&db, "shopping", &[]).unwrap();
        assert_eq!(crate::plugins::host_api::kv_rows(&db, "shopping").unwrap(), vec![]);
        parts.apply(&store, "shopping", Some(&first)).unwrap();
        assert_eq!(crate::plugins::host_api::kv_rows(&db, "shopping").unwrap(), rows);
        assert_eq!(DataParts::read(&store, &first).unwrap().summary(), "записей: 1");
    }
}
