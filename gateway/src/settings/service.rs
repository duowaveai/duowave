//! The gateway's stored settings, typed by their definitions (definitions.rs): a global value, for `user` settings a
//! person's own value on top, for `app` settings an app's own value on top (`app_settings`). Every write publishes `settings.changed` (without the value: some are
//! sensitive). Reads go to the database each time — it is local and small, and nothing goes stale between modules
//! that read the same key.

use std::collections::BTreeMap;
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::settings::{SettingDefinition, SettingScope};
use crate::platform::http::ApiError;
use crate::storage::db::Db;
use crate::utils::time::now_iso;

#[derive(Debug, Clone, PartialEq)]
pub struct SettingEntry<T> {
    pub value: T,
    /// Still the value the gateway seeded, nobody has set one.
    pub is_default: bool,
}

pub struct SettingsService {
    db: Arc<Db>,
    events: EventBus,
}

impl SettingsService {
    pub fn new(db: Arc<Db>, events: EventBus) -> SettingsService {
        SettingsService { db, events }
    }

    /// The value for a person: theirs, then the global one; None when neither is stored or parses.
    pub fn get<T>(&self, definition: &SettingDefinition<T>, user_id: Option<&str>) -> Option<T> {
        if let (Some(user_id), SettingScope::User) = (user_id, definition.scope) {
            if let Some(own) = self.get_user(definition, user_id) {
                return Some(own);
            }
        }
        self.entry(definition).map(|entry| entry.value)
    }

    /// The global value and whether it is still the seeded default.
    pub fn entry<T>(&self, definition: &SettingDefinition<T>) -> Option<SettingEntry<T>> {
        self.try_entry(definition).unwrap_or_else(|error| {
            tracing::warn!("setting {}: not read: {error}", definition.key);
            None
        })
    }

    /// The same, with a failed read as an error: for whoever must not take it for «never set» (a value that
    /// narrows rights).
    pub fn try_entry<T>(&self, definition: &SettingDefinition<T>) -> rusqlite::Result<Option<SettingEntry<T>>> {
        let row: Option<(String, bool)> = self.db.with(|c| {
            c.query_row("SELECT value, is_default FROM settings WHERE key = ?1", params![definition.key], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
        })?;
        let Some((text, is_default)) = row else { return Ok(None) };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { return Ok(None) };
        Ok((definition.parse)(&value).map(|value| SettingEntry { value, is_default }))
    }

    /// The global value, or the database's error: for a caller that must not take a failed read for "not set" (the
    /// brain keeps its last choice rather than fall back to .env), as the Node gateway's `peek` threw.
    pub fn peek<T>(&self, definition: &SettingDefinition<T>) -> Result<Option<T>, ApiError> {
        Ok(self.try_entry(definition)?.map(|entry| entry.value))
    }

    /// Whatever is stored under the key, unparsed.
    pub fn raw(&self, key: &str) -> Option<Value> {
        let text: Option<String> = self
            .db
            .with(|c| {
                c.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0)).optional()
            })
            .ok()
            .flatten();
        text.and_then(|text| serde_json::from_str(&text).ok())
    }

    pub fn set<T: Serialize>(
        &self,
        definition: &SettingDefinition<T>,
        value: &T,
        actor: Option<Actor>,
    ) -> Result<(), ApiError> {
        let json = serde_json::to_string(value)?;
        self.db.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, ?3)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value, is_default = 0, updated_at = excluded.updated_at",
                params![definition.key, json, now_iso()],
            )
        })?;
        self.events.emit(GatewayEvent::SettingsChanged { key: definition.key.to_string(), user_id: None, actor });
        Ok(())
    }

    /// Whether a global value is stored under the key at all, parsed or not: a failed read is the database's error,
    /// never «not stored» (a seed must not take a broken read for a fresh gateway).
    pub fn has(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.with(|c| {
            c.query_row("SELECT 1 FROM settings WHERE key = ?1", params![key], |_| Ok(()))
                .optional()
                .map(|found| found.is_some())
        })
    }

    /// Removes the global value: the gateway reads it as never set.
    pub fn forget<T>(&self, definition: &SettingDefinition<T>) -> Result<(), ApiError> {
        self.db.with(|c| c.execute("DELETE FROM settings WHERE key = ?1", params![definition.key]))?;
        Ok(())
    }

    /// The value a fresh gateway starts from: written only while nothing is stored.
    pub fn seed<T: Serialize>(&self, definition: &SettingDefinition<T>, value: &T) -> Result<(), ApiError> {
        let json = serde_json::to_string(value)?;
        self.db.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 1, ?3) ON CONFLICT (key) DO NOTHING",
                params![definition.key, json, now_iso()],
            )
        })?;
        Ok(())
    }

    pub fn get_user<T>(&self, definition: &SettingDefinition<T>, user_id: &str) -> Option<T> {
        let text: Option<String> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT value FROM user_settings WHERE user_id = ?1 AND key = ?2",
                    params![user_id, definition.key],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten();
        let value: Value = serde_json::from_str(&text?).ok()?;
        (definition.parse)(&value)
    }

    /// A person's own value; None removes it, and the global one applies again.
    pub fn set_user<T: Serialize>(
        &self,
        definition: &SettingDefinition<T>,
        user_id: &str,
        value: Option<&T>,
        actor: Option<Actor>,
    ) -> Result<(), ApiError> {
        if definition.scope != SettingScope::User {
            return Err(ApiError::internal(format!("{} is not a per-person setting", definition.key)));
        }
        match value {
            None => {
                self.db.with(|c| {
                    c.execute(
                        "DELETE FROM user_settings WHERE user_id = ?1 AND key = ?2",
                        params![user_id, definition.key],
                    )
                })?;
            }
            Some(value) => {
                let json = serde_json::to_string(value)?;
                self.db.with(|c| {
                    c.execute(
                        "INSERT INTO user_settings (user_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (user_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                        params![user_id, definition.key, json, now_iso()],
                    )
                })?;
            }
        }
        self.events.emit(GatewayEvent::SettingsChanged {
            key: definition.key.to_string(),
            user_id: Some(user_id.to_string()),
            actor,
        });
        Ok(())
    }
}

impl SettingsService {
    /// An app's own value of a setting; None when it has none or it does not parse.
    pub fn get_app<T>(&self, definition: &SettingDefinition<T>, app_id: &str) -> Option<T> {
        let text: Option<String> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT value FROM app_settings WHERE app_id = ?1 AND key = ?2",
                    params![app_id, definition.key],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten();
        let value: Value = serde_json::from_str(&text?).ok()?;
        (definition.parse)(&value)
    }

    /// The value for an app: its own, then the global one.
    pub fn get_for_app<T>(&self, definition: &SettingDefinition<T>, app_id: &str) -> Option<T> {
        if definition.scope == SettingScope::App {
            if let Some(own) = self.get_app(definition, app_id) {
                return Some(own);
            }
        }
        self.entry(definition).map(|entry| entry.value)
    }

    /// An app's own value; None removes it, and the global one applies again.
    pub fn set_for_app<T: Serialize>(
        &self,
        definition: &SettingDefinition<T>,
        app_id: &str,
        value: Option<&T>,
        actor: Option<Actor>,
    ) -> Result<(), ApiError> {
        if definition.scope != SettingScope::App {
            return Err(ApiError::internal(format!("{} is not a per-app setting", definition.key)));
        }
        match value {
            None => {
                self.db.with(|c| {
                    c.execute(
                        "DELETE FROM app_settings WHERE app_id = ?1 AND key = ?2",
                        params![app_id, definition.key],
                    )
                })?;
            }
            Some(value) => {
                let json = serde_json::to_string(value)?;
                self.db.with(|c| {
                    c.execute(
                        "INSERT INTO app_settings (app_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (app_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                        params![app_id, definition.key, json, now_iso()],
                    )
                })?;
            }
        }
        self.events.emit(GatewayEvent::SettingsChanged { key: definition.key.to_string(), user_id: None, actor });
        Ok(())
    }

    /// Every app's own value of one setting, by app id: whoever decides for many apps at once reads them in one go.
    /// Values that do not parse are left out.
    pub fn all_for_apps<T>(&self, definition: &SettingDefinition<T>) -> BTreeMap<String, T> {
        let rows: Vec<(String, String)> = self
            .db
            .with(|c| {
                let mut statement =
                    c.prepare("SELECT app_id, value FROM app_settings WHERE key = ?1 ORDER BY app_id")?;
                let rows = statement.query_map(params![definition.key], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .unwrap_or_else(|error| {
                tracing::warn!("setting {}: the apps' values not read: {error}", definition.key);
                Vec::new()
            });
        rows.into_iter()
            .filter_map(|(app, text)| {
                let value: Value = serde_json::from_str(&text).ok()?;
                Some((app, (definition.parse)(&value)?))
            })
            .collect()
    }

    /// An app's own values, unparsed, by key.
    pub fn list_for_app(&self, app_id: &str) -> BTreeMap<String, Value> {
        let rows: Vec<(String, String)> = self
            .db
            .with(|c| {
                let mut statement = c.prepare("SELECT key, value FROM app_settings WHERE app_id = ?1 ORDER BY key")?;
                let rows = statement.query_map(params![app_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .unwrap_or_default();
        rows.into_iter().filter_map(|(key, text)| Some((key, serde_json::from_str(&text).ok()?))).collect()
    }

    /// An app's own values all gone: its data went for good. How many there were.
    pub fn forget_app(&self, app_id: &str) -> Result<usize, ApiError> {
        let removed = self.db.with(|c| c.execute("DELETE FROM app_settings WHERE app_id = ?1", params![app_id]))?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::definitions::{KEEP_REQUESTS, ROUTES, VOICE_PROVIDER};

    fn service() -> SettingsService {
        SettingsService::new(Arc::new(Db::memory()), EventBus::new())
    }

    #[test]
    fn seeds_once_and_a_write_clears_the_default() {
        let settings = service();
        settings.seed(&KEEP_REQUESTS, &30).unwrap();
        settings.seed(&KEEP_REQUESTS, &5).unwrap();
        assert_eq!(settings.entry(&KEEP_REQUESTS), Some(SettingEntry { value: 30, is_default: true }));
        settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        assert_eq!(settings.entry(&KEEP_REQUESTS), Some(SettingEntry { value: 7, is_default: false }));
        settings.forget(&KEEP_REQUESTS).unwrap();
        assert_eq!(settings.get(&KEEP_REQUESTS, None), None);
    }

    #[test]
    fn a_persons_value_over_the_global_one() {
        let settings = service();
        settings
            .db
            .with(|c| {
                c.execute("INSERT INTO users (id, name, created_at, updated_at) VALUES ('u1', 'Аня', 'now', 'now')", [])
            })
            .unwrap();
        settings.set(&VOICE_PROVIDER, &"yandex".to_string(), None).unwrap();
        settings.set_user(&VOICE_PROVIDER, "u1", Some(&"own".to_string()), None).unwrap();
        assert_eq!(settings.get(&VOICE_PROVIDER, Some("u1")).as_deref(), Some("own"));
        assert_eq!(settings.get(&VOICE_PROVIDER, Some("u2")).as_deref(), Some("yandex"));
        settings.set_user::<String>(&VOICE_PROVIDER, "u1", None, None).unwrap();
        assert_eq!(settings.get(&VOICE_PROVIDER, Some("u1")).as_deref(), Some("yandex"));
        assert!(settings.set_user(&ROUTES, "u1", Some(&Default::default()), None).is_err(), "routes are global");
    }

    #[test]
    fn an_apps_value_over_the_global_one() {
        use crate::core::settings::{as_string, SettingDefinition, SettingScope};
        const RULE: SettingDefinition<String> = SettingDefinition {
            key: "test.rule",
            scope: SettingScope::App,
            description: "",
            sensitive: false,
            parse: as_string,
        };
        let settings = service();
        settings.set(&RULE, &"auto".to_string(), None).unwrap();
        settings.set_for_app(&RULE, "maps", Some(&"direct".to_string()), None).unwrap();
        settings.set_for_app(&RULE, "system:alice", Some(&"c1".to_string()), None).unwrap();
        assert_eq!(settings.get_for_app(&RULE, "maps").as_deref(), Some("direct"));
        assert_eq!(settings.get_for_app(&RULE, "radio").as_deref(), Some("auto"));
        assert_eq!(settings.get_app(&RULE, "radio"), None);
        let all = settings.all_for_apps(&RULE);
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("system:alice").map(String::as_str), Some("c1"));
        assert_eq!(settings.list_for_app("maps").get("test.rule"), Some(&serde_json::json!("direct")));
        settings.set_for_app::<String>(&RULE, "maps", None, None).unwrap();
        assert_eq!(settings.get_for_app(&RULE, "maps").as_deref(), Some("auto"));
        assert_eq!(settings.forget_app("system:alice").unwrap(), 1);
        assert!(settings.all_for_apps(&RULE).is_empty());
        assert!(settings.set_for_app(&KEEP_REQUESTS, "maps", Some(&3), None).is_err(), "a global setting");
    }

    #[test]
    fn a_stored_value_that_does_not_parse_reads_as_absent() {
        let settings = service();
        settings
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES ('requests_keep_days', '\"x\"', 'now')",
                    [],
                )
            })
            .unwrap();
        assert_eq!(settings.get(&KEEP_REQUESTS, None), None);
        assert_eq!(settings.raw("requests_keep_days"), Some(serde_json::json!("x")));
    }
}
