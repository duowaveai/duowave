//! The gateway's secrets — model keys, MCP tokens and variables, OAuth tokens, the plugins' private settings — sealed
//! in the database (secret_box.rs), open in memory only. Ids are paths: `llm:<provider>:key`, `mcp:<server>:env:NAME`,
//! `plugin:<package>:<key>`. No API returns a value; the panel sees masks. Values that do not open (the key file was
//! lost) read as unset: the panel asks for them again, nothing breaks.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use rusqlite::params;
use serde::Serialize;

use super::secret_box::{hint, load_key, open, seal, Key, Sealed};
use crate::platform::http::ApiError;
use crate::storage::db::Db;
use crate::utils::time::now_iso;

/// The prefix of the plugins' private settings (`"secret": true` in a manifest): a key, a private link.
pub const PLUGIN_SECRETS: &str = "plugin:";

/// Where a plugin's private setting lives in the safe: `plugin:<package>:<key>`.
pub fn plugin_secret(package: &str, key: &str) -> String {
    format!("{PLUGIN_SECRETS}{package}:{key}")
}

/// The prefix of one plugin's private settings: what its removal takes, what its copy copies.
pub fn plugin_secrets(package: &str) -> String {
    format!("{PLUGIN_SECRETS}{package}:")
}

/// The shortest value `redact` hides: a plugin's key may be short (a PIN, a four-digit code), the gateway's own keys
/// are long, and hiding a short common word of theirs would garble the text.
fn hidden_from(id: &str) -> usize {
    if id.starts_with(PLUGIN_SECRETS) {
        4
    } else {
        8
    }
}

/// What the panel may know of a secret: whether it is set and its last characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretMask {
    pub set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub struct SecretsService {
    db: Arc<Db>,
    key: Key,
    values: RwLock<BTreeMap<String, String>>,
}

impl SecretsService {
    /// The secrets of the database, opened with the key in `state_dir` (made when there is none).
    pub fn new(db: Arc<Db>, state_dir: Option<&Path>) -> std::io::Result<SecretsService> {
        let loaded = load_key(state_dir)?;
        let rows: Vec<(String, Sealed)> = db
            .with(|c| {
                let mut statement = c.prepare("SELECT id, ciphertext, iv, tag FROM secrets")?;
                let rows = statement.query_map([], |row| {
                    Ok((row.get(0)?, Sealed { ciphertext: row.get(1)?, iv: row.get(2)?, tag: row.get(3)? }))
                })?;
                rows.collect()
            })
            .unwrap_or_default();
        let mut values = BTreeMap::new();
        let mut unreadable = 0;
        for (id, sealed) in rows {
            match open(&loaded.key, &id, &sealed) {
                Some(value) => {
                    values.insert(id, value);
                }
                None => unreadable += 1,
            }
        }
        if unreadable > 0 {
            tracing::warn!(
                "{unreadable} secret(s) do not open with {}{}: enter them again in the panel",
                loaded.file.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| "this key".into()),
                if loaded.created { " (made just now)" } else { "" }
            );
        }
        Ok(SecretsService { db, key: loaded.key, values: RwLock::new(values) })
    }

    pub fn get(&self, id: &str) -> Option<String> {
        self.values.read().unwrap().get(id).cloned()
    }

    pub fn has(&self, id: &str) -> bool {
        self.values.read().unwrap().contains_key(id)
    }

    pub fn mask(&self, id: &str) -> SecretMask {
        match self.values.read().unwrap().get(id) {
            Some(value) => SecretMask { set: true, hint: Some(hint(value)) },
            None => SecretMask { set: false, hint: None },
        }
    }

    /// The ids under a prefix (`mcp:home:`), for a connector's whole set.
    pub fn ids(&self, prefix: &str) -> Vec<String> {
        self.values.read().unwrap().keys().filter(|id| id.starts_with(prefix)).cloned().collect()
    }

    /// Stores a value; an empty one removes it.
    pub fn set(&self, id: &str, value: Option<&str>) -> Result<(), ApiError> {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            return self.delete(id);
        };
        let sealed = seal(&self.key, id, value);
        self.db.with(|c| {
            c.execute(
                "INSERT INTO secrets (id, ciphertext, iv, tag, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (id) DO UPDATE SET ciphertext = excluded.ciphertext, iv = excluded.iv, tag = excluded.tag,
                 updated_at = excluded.updated_at",
                params![id, sealed.ciphertext, sealed.iv, sealed.tag, now_iso()],
            )
        })?;
        self.values.write().unwrap().insert(id.to_string(), value.to_string());
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.db.with(|c| c.execute("DELETE FROM secrets WHERE id = ?1", params![id]))?;
        self.values.write().unwrap().remove(id);
        Ok(())
    }

    /// Removes every secret under a prefix: a deleted provider or server takes its keys with it.
    pub fn delete_all(&self, prefix: &str) -> Result<(), ApiError> {
        self.db.with(|c| {
            c.execute(
                "DELETE FROM secrets WHERE substr(id, 1, ?1) = ?2",
                params![prefix.chars().count() as i64, prefix],
            )
        })?;
        self.values.write().unwrap().retain(|id, _| !id.starts_with(prefix));
        Ok(())
    }

    /// Hides every known secret value in a text before it reaches a log or the journal: the gateway's keys from 8
    /// characters, the plugins' from 4. The longest first, so a value inside another does not leave the rest of it.
    pub fn redact(&self, text: &str) -> String {
        let values = self.values.read().unwrap();
        let mut hidden: Vec<&str> = values
            .iter()
            .filter(|(id, value)| value.chars().count() >= hidden_from(id))
            .map(|(_, value)| value.as_str())
            .collect();
        hidden.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut out = text.to_string();
        for value in hidden {
            if out.contains(value) {
                out = out.replace(value, "•••");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_values_sealed_and_open_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(dir.path())).unwrap());
        let secrets = SecretsService::new(db.clone(), Some(dir.path())).unwrap();
        secrets.set("llm:claude:key", Some("sk-ant-0123456789")).unwrap();
        secrets.set("mcp:home:env:TOKEN", Some("token-abcdefgh")).unwrap();
        secrets.set("mcp:home:header:X", Some("value-123456")).unwrap();
        let stored: String = db
            .with(|c| c.query_row("SELECT ciphertext FROM secrets WHERE id = 'llm:claude:key'", [], |r| r.get(0)))
            .unwrap();
        assert!(!stored.contains("sk-ant"));
        let again = SecretsService::new(db, Some(dir.path())).unwrap();
        assert_eq!(again.get("llm:claude:key").as_deref(), Some("sk-ant-0123456789"));
        assert_eq!(again.mask("llm:claude:key"), SecretMask { set: true, hint: Some("…6789".into()) });
        assert_eq!(again.ids("mcp:home:"), vec!["mcp:home:env:TOKEN", "mcp:home:header:X"]);
        again.delete_all("mcp:home:").unwrap();
        assert!(again.ids("mcp:").is_empty());
        again.set("llm:claude:key", Some("")).unwrap();
        assert!(!again.has("llm:claude:key"));
    }

    #[test]
    fn redacts_known_values() {
        let secrets = SecretsService::new(Arc::new(Db::memory()), None).unwrap();
        secrets.set("a", Some("supersecretvalue")).unwrap();
        secrets.set("b", Some("short")).unwrap();
        assert_eq!(secrets.redact("key=supersecretvalue short"), "key=••• short");
    }

    /// A plugin's key is hidden from 4 characters: a PIN in its journal is not shown to the workshop's agent.
    #[test]
    fn redacts_a_plugins_short_key_too() {
        let secrets = SecretsService::new(Arc::new(Db::memory()), None).unwrap();
        secrets.set(&plugin_secret("todo", "token"), Some("4721")).unwrap();
        secrets.set(&plugin_secret("todo", "pin"), Some("472")).unwrap();
        secrets.set("llm:claude:key", Some("sk-4721-long-enough")).unwrap();
        assert_eq!(plugin_secret("todo", "token"), "plugin:todo:token");
        assert_eq!(secrets.ids(&plugin_secrets("todo")), vec!["plugin:todo:pin", "plugin:todo:token"]);
        assert_eq!(
            secrets.redact("pin 4721 with key sk-4721-long-enough, 472 left"),
            "pin ••• with key •••, 472 left",
            "the longer key first: the short one inside it does not cut it apart; 3 characters stay"
        );
    }
}
