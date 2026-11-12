//! The gateway's one database: a SQLite file in STATE_DIR (`duowave.sqlite`), or memory when there is none (tests,
//! STATE_DIR set empty). Opening it brings it to the current schema: the SQL migrations in `gateway/migrations/` are
//! built into the program and applied in order, each in its own transaction, and recorded in `schema_migrations`
//! with the SHA-256 of their text. A database with data gets a consistent copy (`VACUUM INTO`) in `backups/` before
//! any pending migration runs, so a migration that goes wrong never costs the household its people and settings.
//!
//! Queries are synchronous and short (the data is small and local), as better-sqlite3's were in the Node gateway:
//! `db.with(|conn| …)` holds the one connection for the duration of the closure.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use crate::utils::{files::owner_only, time::now_iso};

/// A migration built into the program.
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// The database file's name in STATE_DIR.
pub const DB_FILE: &str = "duowave.sqlite";

/// Copies kept before migrations, newest first; older ones are removed.
const KEPT_BACKUPS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database: {0}")]
    Io(#[from] std::io::Error),
    #[error("migration {version} ({name}) was changed after it was applied: add a new migration instead")]
    Changed { version: u32, name: String },
}

pub struct Db {
    connection: Mutex<Connection>,
    file: Option<PathBuf>,
    applied: usize,
    backup: Option<PathBuf>,
}

impl Db {
    /// The database in `state_dir`, created and migrated; in memory without a folder.
    pub fn open(state_dir: Option<&Path>) -> Result<Db, DbError> {
        let file = match state_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                Some(dir.join(DB_FILE))
            }
            None => None,
        };
        let mut connection = match &file {
            Some(file) => Connection::open(file)?,
            None => Connection::open_in_memory()?,
        };
        if file.is_some() {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_millis(5000))?;
        let (applied, backup) = migrate(&mut connection, file.as_deref())?;
        if let Some(file) = &file {
            protect(file);
        }
        Ok(Db { connection: Mutex::new(connection), file, applied, backup })
    }

    /// A fresh database in memory, migrated: tests.
    pub fn memory() -> Db {
        Db::open(None).expect("an in-memory database opens")
    }

    /// The file; None in memory.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// How many migrations the opening applied.
    pub fn applied_migrations(&self) -> usize {
        self.applied
    }

    /// The copy taken before those migrations, when the database had data.
    pub fn backup(&self) -> Option<&Path> {
        self.backup.as_deref()
    }

    /// Runs `work` on the connection.
    pub fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        work(&self.lock())
    }

    /// Runs `work` in a transaction: committed when it returns Ok, rolled back otherwise.
    pub fn transaction<T>(&self, work: impl FnOnce(&Transaction) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the connection leaves SQLite consistent (the transaction rolls back): keep going.
        self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn checksum(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

fn migrate(connection: &mut Connection, file: Option<&Path>) -> Result<(usize, Option<PathBuf>), DbError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        )",
    )?;
    let applied: Vec<(u32, String, String)> = {
        let mut statement =
            connection.prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (version, name, sum) in &applied {
        match MIGRATIONS.iter().find(|migration| migration.version == *version) {
            Some(migration) if checksum(migration.sql) != *sum => {
                return Err(DbError::Changed { version: *version, name: name.clone() });
            }
            Some(_) => {}
            None => tracing::warn!(
                "the database has migration {version} ({name}), which this version does not know: a newer duowave \
                 made it; this one reads only the columns it knows"
            ),
        }
    }
    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|migration| !applied.iter().any(|(version, _, _)| *version == migration.version))
        .collect();
    let backup = match file {
        Some(file) if !pending.is_empty() && !applied.is_empty() => Some(backup_before(connection, file)?),
        _ => None,
    };
    for migration in &pending {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration.sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
            params![migration.version, migration.name, checksum(migration.sql), now_iso()],
        )?;
        transaction.commit()?;
        tracing::info!("database: migration {} ({}) applied", migration.version, migration.name);
    }
    Ok((pending.len(), backup))
}

fn backup_before(connection: &Connection, file: &Path) -> Result<PathBuf, DbError> {
    let dir = file.parent().unwrap_or(Path::new(".")).join("backups");
    std::fs::create_dir_all(&dir)?;
    owner_only(&dir);
    let stamp = now_iso().replace([':', '.'], "-");
    let target = dir.join(format!("duowave-{stamp}.sqlite"));
    connection.execute("VACUUM INTO ?1", params![target.to_string_lossy()])?;
    owner_only(&target);
    let mut old: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("duowave-") && name.ends_with(".sqlite"))
        })
        .collect();
    old.sort();
    old.reverse();
    for path in old.into_iter().skip(KEPT_BACKUPS) {
        let _ = std::fs::remove_file(path);
    }
    Ok(target)
}

/// The database, its journal files and its copies are readable by the owner only: they hold people's names, the
/// journal of what was said and the secrets' ciphertext.
fn protect(file: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", file.display()));
        if path.exists() {
            owner_only(&path);
        }
    }
}

/// A single value or None: `SELECT value FROM … WHERE …`.
pub fn optional<T>(result: rusqlite::Result<T>) -> rusqlite::Result<Option<T>> {
    result.optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_a_new_database_to_every_table() {
        let db = Db::memory();
        assert_eq!(db.applied_migrations(), MIGRATIONS.len());
        let tables: Vec<String> = db
            .with(|c| {
                let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
                let names = s.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
                Ok(names)
            })
            .unwrap();
        for table in [
            "api_keys",
            "asr_models",
            "audit_log",
            "characters",
            "llm_providers",
            "mcp_servers",
            "memory_aliases",
            "memory_changes",
            "memory_entities",
            "memory_episodes",
            "memory_facts",
            "memory_notes",
            "memory_visible_to",
            "packages",
            "plugin_kv",
            "requests",
            "schema_migrations",
            "secrets",
            "settings",
            "sleep_runs",
            "state",
            "stations",
            "user_settings",
            "users",
            "voice_models",
            "voices",
        ] {
            assert!(tables.contains(&table.to_string()), "{table} in {tables:?}");
        }
    }

    #[test]
    fn reopening_applies_nothing_and_a_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let first = Db::open(Some(dir.path())).unwrap();
        assert_eq!(first.applied_migrations(), MIGRATIONS.len());
        first
            .with(|c| c.execute("INSERT INTO settings (key, value, updated_at) VALUES ('k', '1', 'now')", []))
            .unwrap();
        drop(first);
        let again = Db::open(Some(dir.path())).unwrap();
        assert_eq!(again.applied_migrations(), 0);
        assert!(again.backup().is_none(), "nothing pending, nothing copied");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(DB_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn refuses_a_migration_changed_after_it_was_applied() {
        let dir = tempfile::tempdir().unwrap();
        drop(Db::open(Some(dir.path())).unwrap());
        let connection = Connection::open(dir.path().join(DB_FILE)).unwrap();
        connection.execute("UPDATE schema_migrations SET checksum = 'other' WHERE version = 1", []).unwrap();
        drop(connection);
        assert!(matches!(Db::open(Some(dir.path())), Err(DbError::Changed { version: 1, .. })));
    }

    /// A household's database as 0.5.x left it — migrations 1–5 with their checksums, people's settings, packages, a
    /// station and its journal — opened by this build: the newer migrations apply one after another, a copy is taken
    /// first, nothing the household had is lost, and the next start applies nothing.
    #[test]
    fn upgrades_a_database_from_0_5_keeping_its_data() {
        const AT_0_5: u32 = 5;
        let dir = tempfile::tempdir().unwrap();
        {
            let mut connection = Connection::open(dir.path().join(DB_FILE)).unwrap();
            connection.pragma_update(None, "foreign_keys", "ON").unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (
                        version INTEGER PRIMARY KEY NOT NULL, name TEXT NOT NULL, checksum TEXT NOT NULL,
                        applied_at TEXT NOT NULL)",
                )
                .unwrap();
            for migration in MIGRATIONS.iter().filter(|migration| migration.version <= AT_0_5) {
                let transaction = connection.transaction().unwrap();
                transaction.execute_batch(migration.sql).unwrap();
                transaction
                    .execute(
                        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, 'then')",
                        params![migration.version, migration.name, checksum(migration.sql)],
                    )
                    .unwrap();
                transaction.commit().unwrap();
            }
            connection
                .execute_batch(
                    "INSERT INTO settings (key, value, updated_at) VALUES ('brain', '{\"effort\":\"low\"}', 'then');
                     INSERT INTO packages (id, enabled, version, updated_at, source, verified)
                         VALUES ('weather', 1, '1.2.0', 'then', 'catalog', 1);
                     INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES ('weather', 'city', '\"Тверь\"', 'then');
                     INSERT INTO stations (device, name, first_seen, last_seen, firmware)
                         VALUES ('dev-1', 'Кухня', 'then', 'then', '1.0');
                     INSERT INTO requests (id, device, kind, mode, status, started_at)
                         VALUES ('r-1', 'dev-1', 'voice', 'own', 'done', 'then');",
                )
                .unwrap();
        }

        let db = Db::open(Some(dir.path())).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(db.applied_migrations(), MIGRATIONS.len() - AT_0_5 as usize);
        assert!(db.backup().is_some_and(Path::exists), "a copy before the migrations");
        let (versions, kept, memory) = db
            .with(|c| {
                let mut s = c.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
                let versions: Vec<u32> = s.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
                let kept: (String, String, String, String, String) = c.query_row(
                    "SELECT (SELECT value FROM settings WHERE key = 'brain'),
                            (SELECT version || ' ' || source FROM packages WHERE id = 'weather'),
                            (SELECT value FROM plugin_kv WHERE plugin_id = 'weather'),
                            (SELECT name || ' ' || firmware FROM stations WHERE device = 'dev-1'),
                            (SELECT status FROM requests WHERE id = 'r-1')",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )?;
                let memory: i64 = c.query_row(
                    "SELECT count(*) FROM memory_entities JOIN memory_episodes ON reflected_run IS NULL",
                    [],
                    |row| row.get(0),
                )?;
                Ok((versions, kept, memory))
            })
            .unwrap();
        assert_eq!(versions, (1..=MIGRATIONS.len() as u32).collect::<Vec<_>>(), "one after another, none skipped");
        assert_eq!(
            kept,
            (
                "{\"effort\":\"low\"}".to_string(),
                "1.2.0 catalog".to_string(),
                "\"Тверь\"".to_string(),
                "Кухня 1.0".to_string(),
                "done".to_string()
            )
        );
        assert_eq!(memory, 0, "the new tables are there and empty");
        drop(db);
        let again = Db::open(Some(dir.path())).unwrap();
        assert_eq!(again.applied_migrations(), 0);
    }

    #[test]
    fn copies_a_database_with_data_before_a_pending_migration() {
        let dir = tempfile::tempdir().unwrap();
        drop(Db::open(Some(dir.path())).unwrap());
        // As if this build brought migration 1 anew to a database that has another one applied: forget 1 and its
        // tables, keep a record of a (future) migration so the database counts as one with data.
        let connection = Connection::open(dir.path().join(DB_FILE)).unwrap();
        connection
            .execute_batch(
                "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (999, 'future', 'x', 'now');
                 DELETE FROM schema_migrations WHERE version = 1;
                 DROP TABLE user_settings; DROP TABLE users; DROP TABLE voices; DROP TABLE voice_models;
                 DROP TABLE state; DROP TABLE settings; DROP TABLE api_keys; DROP TABLE audit_log;
                 DROP TABLE asr_models; DROP TABLE characters; DROP TABLE stations; DROP TABLE requests;
                 DROP TABLE secrets; DROP TABLE llm_providers; DROP TABLE mcp_servers;",
            )
            .unwrap();
        drop(connection);
        let db = Db::open(Some(dir.path())).unwrap_or_else(|error| panic!("{error}"));
        let backup = db.backup().expect("a copy before the migration");
        assert!(backup.exists());
    }
}
