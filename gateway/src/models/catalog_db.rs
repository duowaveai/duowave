//! The catalog as database rows: the showcase seeds it once, the open registry upserts into it, and choices refer to
//! the rows by id — names repeat (two Alans in two languages), ids do not. Nothing duplicates: every write is an
//! upsert on the primary key, and a registry voice a showcase card already carries is not written at all. On the
//! gateway's one database (in memory without STATE_DIR, as in tests).
//!
//! The catalog's tables (voices, recognition models, Alisa's character cards) are one repository for the model shop
//! and the persona: this part is in every build, the station's too — the persona reads its character cards here.

use std::sync::Arc;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::platform::http::ApiError;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// A voice model as catalog data: what downloads, and the voices it keeps inside (Silero five, Piper one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelRecord {
    pub id: String,
    /// silero or piper.
    pub engine: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// builtin (the showcase) or registry (the open Piper voices).
    pub source: String,
    pub voices: Vec<VoiceEntry>,
}

/// A voice inside a model: what a synthesis request asks for (`name`), and its short word for lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrModelRecord {
    pub id: String,
    /// What the recognition server loads (v3_e2e_rnnt, whisper-small) — the card's id is its own.
    pub model: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterRecord {
    pub id: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub text: String,
}

/// An optional column as the catalog reads it: an empty text is no value.
fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

pub struct CatalogDb {
    db: Arc<Db>,
}

impl CatalogDb {
    pub fn new(db: Arc<Db>) -> CatalogDb {
        CatalogDb { db }
    }

    /// Voice models with their voices, in the order they were first written: the showcase rows first, the registry
    /// after.
    pub fn list_voice_models(&self) -> Result<Vec<VoiceModelRecord>, ApiError> {
        let (mut models, voices) = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, engine, title, description, badge, license, locale, quality, size, url, file, source
                 FROM voice_models ORDER BY rowid",
            )?;
            let models = statement
                .query_map([], |row| {
                    Ok(VoiceModelRecord {
                        id: row.get(0)?,
                        engine: row.get(1)?,
                        title: row.get(2)?,
                        description: row.get(3)?,
                        badge: present(row.get(4)?),
                        license: present(row.get(5)?),
                        locale: present(row.get(6)?),
                        quality: present(row.get(7)?),
                        size: present(row.get(8)?),
                        url: present(row.get(9)?),
                        file: present(row.get(10)?),
                        source: row.get(11)?,
                        voices: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut statement = c.prepare("SELECT model_id, name, short FROM voices ORDER BY rowid")?;
            let voices = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, VoiceEntry { name: row.get(1)?, short: present(row.get(2)?) }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((models, voices))
        })?;
        let mut by_model: std::collections::HashMap<String, Vec<VoiceEntry>> = std::collections::HashMap::new();
        for (model_id, voice) in voices {
            by_model.entry(model_id).or_default().push(voice);
        }
        for model in &mut models {
            model.voices = by_model.remove(&model.id).unwrap_or_default();
        }
        Ok(models)
    }

    /// Upserts by id: re-seeding the showcase or refreshing the registry never duplicates a row.
    pub fn save_voice_models(&self, entries: &[VoiceModelRecord]) -> Result<(), ApiError> {
        if entries.is_empty() {
            return Ok(());
        }
        let at = now_iso();
        self.db.transaction(|t| {
            let mut upsert = t.prepare(
                "INSERT INTO voice_models
                     (id, engine, title, description, badge, license, locale, quality, size, url, file, source, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (id) DO UPDATE SET
                     engine = excluded.engine, title = excluded.title, description = excluded.description,
                     badge = excluded.badge, license = excluded.license, locale = excluded.locale,
                     quality = excluded.quality, size = excluded.size, url = excluded.url, file = excluded.file,
                     updated_at = excluded.updated_at",
            )?;
            for entry in entries {
                upsert.execute(params![
                    entry.id,
                    entry.engine,
                    entry.title,
                    entry.description,
                    entry.badge,
                    entry.license,
                    entry.locale,
                    entry.quality,
                    entry.size,
                    entry.url,
                    entry.file,
                    entry.source,
                    at,
                ])?;
            }
            // The model's voices are its whole truth: replaced, not merged — no voice outlives its model's edit.
            let mut clear = t.prepare("DELETE FROM voices WHERE model_id = ?1")?;
            for entry in entries {
                clear.execute(params![entry.id])?;
            }
            let mut insert =
                t.prepare("INSERT INTO voices (id, model_id, name, short, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for entry in entries {
                for voice in &entry.voices {
                    insert.execute(params![
                        format!("{}/{}", entry.id, voice.name),
                        entry.id,
                        voice.name,
                        voice.short,
                        at
                    ])?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn list_asr(&self) -> Result<Vec<AsrModelRecord>, ApiError> {
        let rows = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, model, title, description, badge, license, source FROM asr_models ORDER BY rowid",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(AsrModelRecord {
                        id: row.get(0)?,
                        model: row.get(1)?,
                        title: row.get(2)?,
                        description: row.get(3)?,
                        badge: present(row.get(4)?),
                        license: present(row.get(5)?),
                        source: present(row.get(6)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        Ok(rows)
    }

    pub fn save_asr(&self, entries: &[AsrModelRecord]) -> Result<(), ApiError> {
        if entries.is_empty() {
            return Ok(());
        }
        let at = now_iso();
        self.db.transaction(|t| {
            let mut upsert = t.prepare(
                "INSERT INTO asr_models (id, model, title, description, badge, license, source, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (id) DO UPDATE SET
                     model = excluded.model, title = excluded.title, description = excluded.description,
                     badge = excluded.badge, license = excluded.license, source = excluded.source,
                     updated_at = excluded.updated_at",
            )?;
            for entry in entries {
                upsert.execute(params![
                    entry.id,
                    entry.model,
                    entry.title,
                    entry.description,
                    entry.badge,
                    entry.license,
                    entry.source,
                    at
                ])?;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn list_characters(&self) -> Result<Vec<CharacterRecord>, ApiError> {
        let rows = self.db.with(|c| {
            let mut statement =
                c.prepare("SELECT id, title, description, badge, text FROM characters ORDER BY rowid")?;
            let rows = statement
                .query_map([], |row| {
                    Ok(CharacterRecord {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        badge: present(row.get(3)?),
                        text: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        Ok(rows)
    }

    pub fn save_characters(&self, entries: &[CharacterRecord]) -> Result<(), ApiError> {
        if entries.is_empty() {
            return Ok(());
        }
        let at = now_iso();
        self.db.transaction(|t| {
            let mut upsert = t.prepare(
                "INSERT INTO characters (id, title, description, badge, text, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (id) DO UPDATE SET
                     title = excluded.title, description = excluded.description, badge = excluded.badge,
                     text = excluded.text, updated_at = excluded.updated_at",
            )?;
            for entry in entries {
                upsert.execute(params![entry.id, entry.title, entry.description, entry.badge, entry.text, at])?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, source: &str, voices: &[&str]) -> VoiceModelRecord {
        VoiceModelRecord {
            id: id.into(),
            engine: "piper".into(),
            title: format!("Title {id}"),
            description: "d".into(),
            badge: None,
            license: Some(String::new()),
            locale: Some("ru_RU".into()),
            quality: None,
            size: None,
            url: None,
            file: Some(format!("{id}.onnx")),
            source: source.into(),
            voices: voices.iter().map(|name| VoiceEntry { name: name.to_string(), short: None }).collect(),
        }
    }

    #[test]
    fn upserts_by_id_and_replaces_a_models_voices() {
        let catalog = CatalogDb::new(Arc::new(Db::memory()));
        catalog.save_voice_models(&[record("a", "builtin", &["one", "two"]), record("b", "builtin", &[])]).unwrap();
        catalog.save_voice_models(&[record("c", "registry", &["three"])]).unwrap();
        let mut again = record("a", "builtin", &["two"]);
        again.title = "Renamed".into();
        catalog.save_voice_models(&[again]).unwrap();
        let models = catalog.list_voice_models().unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"], "first-written order");
        assert_eq!(models[0].title, "Renamed");
        assert_eq!(models[0].voices, vec![VoiceEntry { name: "two".into(), short: None }], "replaced, not merged");
        assert_eq!(models[0].license, None, "an empty text reads as no value");
        assert_eq!(models[2].voices.len(), 1);
    }

    #[test]
    fn keeps_recognition_and_character_cards() {
        let catalog = CatalogDb::new(Arc::new(Db::memory()));
        let card = AsrModelRecord {
            id: "gigaam-v3-e2e".into(),
            model: "v3_e2e_rnnt".into(),
            title: "GigaAM".into(),
            description: "d".into(),
            badge: Some("рекомендуем".into()),
            license: None,
            source: None,
        };
        catalog.save_asr(std::slice::from_ref(&card)).unwrap();
        catalog.save_asr(&[AsrModelRecord { title: "GigaAM v3".into(), ..card.clone() }]).unwrap();
        assert_eq!(catalog.list_asr().unwrap(), vec![AsrModelRecord { title: "GigaAM v3".into(), ..card }]);
        let character = CharacterRecord {
            id: "classic".into(),
            title: "Алиса".into(),
            description: "d".into(),
            badge: None,
            text: "Ты — Алиса".into(),
        };
        catalog.save_characters(std::slice::from_ref(&character)).unwrap();
        catalog.save_characters(std::slice::from_ref(&character)).unwrap();
        assert_eq!(catalog.list_characters().unwrap(), vec![character]);
    }
}
