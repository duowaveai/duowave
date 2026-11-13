//! The packages the gateway knows: those in PACKAGES_DIR (`<id>/<version>/manifest.json`, any code beside it) and
//! the admin's choices in the database (table `packages`: on or off, the version that last started well, where the
//! package came from — the catalog or by hand — whether the catalog vouched for it, the feed it came from and whether
//! that feed hides it: docs/plans/custom-feeds.md).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::params;

use super::manifest::{self, valid_id, Manifest};
use crate::utils::time::now_iso;

/// A manifest bigger than this is not read; code bigger than this is not loaded.
const MAX_MANIFEST: u64 = 64 * 1024;
pub const MAX_CODE: u64 = 1024 * 1024;
/// A WebAssembly module is bigger than a script for the same work.
pub const MAX_WASM: u64 = 4 * 1024 * 1024;

/// A version of a plugin on disk, checked.
#[derive(Debug)]
pub struct Package {
    pub dir: PathBuf,
    pub manifest: Arc<Manifest>,
}

impl Package {
    pub fn version(&self) -> String {
        self.manifest.version.to_string()
    }

    /// Its code, read now: an edit on disk is the next load's.
    pub fn source(&self) -> Result<String, String> {
        let file = self.dir.join("backend.js");
        let size = std::fs::metadata(&file).map_err(|_| "нет backend.js".to_string())?.len();
        if size > MAX_CODE {
            return Err(format!("backend.js больше {} КБ", MAX_CODE / 1024));
        }
        std::fs::read_to_string(&file).map_err(|error| format!("backend.js не читается: {error}"))
    }

    /// Its WebAssembly (`runtime: wasm`), read now.
    pub fn wasm(&self) -> Result<Vec<u8>, String> {
        let file = self.dir.join("backend.wasm");
        let size = std::fs::metadata(&file).map_err(|_| "нет backend.wasm".to_string())?.len();
        if size > MAX_WASM {
            return Err(format!("backend.wasm больше {} МБ", MAX_WASM / 1024 / 1024));
        }
        std::fs::read(&file).map_err(|error| format!("backend.wasm не читается: {error}"))
    }

    /// Its static files (`assets/`): what /media/packages/<id>/ serves.
    pub fn assets(&self) -> PathBuf {
        self.dir.join("assets")
    }
}

/// A version found on disk: its package, or why it cannot be one.
#[derive(Debug, Clone)]
pub enum Candidate {
    Ready(Arc<Package>),
    Broken { version: String, problems: Vec<String> },
}

impl Candidate {
    pub fn version(&self) -> String {
        match self {
            Candidate::Ready(package) => package.version(),
            Candidate::Broken { version, .. } => version.clone(),
        }
    }

    fn order(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version()).ok()
    }
}

/// A version in `dir`, read as the folder `<id>/<version>/` it is (or is about to be): its package, or why not.
pub fn read_package(dir: &Path, id: &str, version: &str) -> Candidate {
    let broken = |problems: Vec<String>| Candidate::Broken { version: version.to_string(), problems };
    let file = dir.join("manifest.json");
    match std::fs::metadata(&file) {
        Ok(meta) if meta.len() > MAX_MANIFEST => {
            return broken(vec![format!("manifest.json больше {} КБ", MAX_MANIFEST / 1024)])
        }
        Ok(_) => {}
        Err(_) => return broken(vec!["нет manifest.json".into()]),
    }
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => return broken(vec![format!("manifest.json не читается: {error}")]),
    };
    let manifest = match manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(error) => return broken(error.problems),
    };
    let mut problems = Vec::new();
    if manifest.id != id {
        problems.push(format!("id «{}» не совпадает с папкой «{id}»", manifest.id));
    }
    if manifest.version.to_string() != version {
        problems.push(format!("version {} не совпадает с папкой «{version}»", manifest.version));
    }
    let contributes = &manifest.contributes;
    let needs_code = !(contributes.intents.is_empty()
        && contributes.tools.is_empty()
        && contributes.media.is_empty()
        && contributes.events.is_empty());
    if needs_code {
        if let Some(file) = manifest.runtime.code_file().filter(|file| !dir.join(file).is_file()) {
            problems.push(format!("нужен {file}: его функции обслуживают то, что плагин даёт"));
        }
    }
    if !problems.is_empty() {
        return broken(problems);
    }
    Candidate::Ready(Arc::new(Package { dir: dir.to_path_buf(), manifest: Arc::new(manifest) }))
}

fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// The newest version that reads as a package: what is installed, whatever runs.
pub fn newest_ready(found: &[Candidate]) -> Option<Arc<Package>> {
    found.iter().find_map(|candidate| match candidate {
        Candidate::Ready(package) => Some(package.clone()),
        Candidate::Broken { .. } => None,
    })
}

/// Every plugin in the folder by id, its versions newest first; a folder that is no plugin's id is left out.
pub fn scan(root: &Path) -> BTreeMap<String, Vec<Candidate>> {
    let mut found = BTreeMap::new();
    for id in names(root) {
        if !valid_id(&id) {
            tracing::warn!(target: "plugins", "{}: «{id}» is no package's id, left out", root.display());
            continue;
        }
        let mut versions: Vec<Candidate> = names(&root.join(&id))
            .into_iter()
            .map(|version| read_package(&root.join(&id).join(&version), &id, &version))
            .collect();
        // Newest first; a folder that is no version at all last.
        versions.sort_by_key(|candidate| std::cmp::Reverse(candidate.order()));
        if !versions.is_empty() {
            found.insert(id, versions);
        }
    }
    found
}

/// Where a package came from.
pub const SOURCE_CATALOG: &str = "catalog";
pub const SOURCE_LOCAL: &str = "local";
/// Built into the gateway (bundled.rs): one that can be switched off, and one that cannot.
pub const SOURCE_BUNDLED: &str = "bundled";
pub const SOURCE_SYSTEM: &str = "system";

/// A package's kind by where it came from (docs/plans/plugins.md, «Экран «Плагины» и три вида пакетов»): the gateway
/// sets it, never the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// In the gateway's build; without it the station does not work as it should.
    System,
    /// In the gateway's build, «Радио» and the like: off at will, updated with the gateway.
    Bundled,
    /// Written in the workshop or put in by hand.
    User,
    /// From the catalog's feed.
    Catalog,
}

impl Origin {
    pub fn of(source: &str) -> Origin {
        match source {
            SOURCE_SYSTEM => Origin::System,
            SOURCE_BUNDLED => Origin::Bundled,
            SOURCE_CATALOG => Origin::Catalog,
            _ => Origin::User,
        }
    }

    pub fn can_disable(self) -> bool {
        self != Origin::System
    }

    /// The built-ins are updated with the gateway: they are switched off, never removed.
    pub fn removable(self) -> bool {
        matches!(self, Origin::User | Origin::Catalog)
    }

    /// Only one's own is changed in place; a built-in or a catalog's gets a copy of one's own.
    pub fn editable(self) -> bool {
        self == Origin::User
    }

    pub fn title(self) -> &'static str {
        match self {
            Origin::System => "системный",
            Origin::Bundled => "встроенный",
            Origin::User => "свой",
            Origin::Catalog => "из каталога",
        }
    }

    /// The same for people, of an «приложение» (docs/ARCHITECTURE.md, «Как называем и где работает»).
    pub fn word(self) -> &'static str {
        match self {
            Origin::System => "системное",
            Origin::Bundled => "встроенное",
            Origin::User => "своё",
            Origin::Catalog => "из каталога",
        }
    }
}

/// What the admin decided about a plugin, and where its package came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub enabled: bool,
    /// The version that last started well.
    pub good: Option<String>,
    /// catalog or local.
    pub source: String,
    /// The catalog vouched for it.
    pub verified: bool,
    /// The address of the feed it was installed from; None — by hand, or from the catalog before 0.6.0.
    pub feed: Option<String>,
    /// Its feed says to hide it (or the own source it came from is closed).
    pub hidden: bool,
}

impl Default for Choice {
    fn default() -> Choice {
        Choice { enabled: false, good: None, source: SOURCE_LOCAL.into(), verified: false, feed: None, hidden: false }
    }
}

pub fn choices(db: &crate::storage::Db) -> HashMap<String, Choice> {
    db.with(|c| {
        let mut statement = c.prepare("SELECT id, enabled, version, source, verified, feed, hidden FROM packages")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Choice {
                    enabled: row.get::<_, i64>(1)? != 0,
                    good: row.get(2)?,
                    source: row.get(3)?,
                    verified: row.get::<_, i64>(4)? != 0,
                    feed: row.get(5)?,
                    hidden: row.get::<_, i64>(6)? != 0,
                },
            ))
        })?;
        rows.collect()
    })
    .unwrap_or_else(|error| {
        tracing::warn!(target: "plugins", "the packages' choices are not read: {error}");
        HashMap::new()
    })
}

/// On or off, and the version that started well when it is known.
pub fn set_enabled(
    db: &crate::storage::Db,
    id: &str,
    enabled: bool,
    good: Option<&str>,
    by: Option<&str>,
) -> rusqlite::Result<()> {
    db.with(|c| {
        c.execute(
            "INSERT INTO packages (id, enabled, version, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, version = COALESCE(excluded.version, packages.version),
             updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![id, enabled as i64, good, now_iso(), by],
        )
        .map(|_| ())
    })
}

/// The version that started well, kept for the next start.
pub fn set_good(db: &crate::storage::Db, id: &str, version: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("UPDATE packages SET version = ?2 WHERE id = ?1", params![id, version]).map(|_| ()))
}

/// A package installed (or updated) by the installer: where it came from and whether the catalog vouched for it. On
/// or off stays as it was; a new package is off. The feed it came from is forgotten (`set_feed` names it), and so is
/// whether a feed hid it: a package put in anew is shown.
pub fn set_installed(
    db: &crate::storage::Db,
    id: &str,
    source: &str,
    verified: bool,
    by: Option<&str>,
) -> rusqlite::Result<()> {
    db.with(|c| {
        c.execute(
            "INSERT INTO packages (id, enabled, version, updated_at, updated_by, source, verified)
             VALUES (?1, 0, NULL, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET source = excluded.source, verified = excluded.verified,
             updated_at = excluded.updated_at, updated_by = excluded.updated_by, feed = NULL, hidden = 0",
            params![id, now_iso(), by, source, verified as i64],
        )
        .map(|_| ())
    })
}

/// The feed a package was installed from: only that feed may hide it.
pub fn set_feed(db: &crate::storage::Db, id: &str, feed: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("UPDATE packages SET feed = ?2 WHERE id = ?1", params![id, feed]).map(|_| ()))
}

/// Hidden or shown by its feed; whether that changed anything.
pub fn set_hidden(db: &crate::storage::Db, id: &str, hidden: bool) -> rusqlite::Result<bool> {
    db.with(|c| {
        c.execute("UPDATE packages SET hidden = ?2 WHERE id = ?1 AND hidden != ?2", params![id, hidden as i64])
            .map(|changed| changed > 0)
    })
}

/// The plugins that are on, the one switched on (or changed) last first: under pressure it goes off first.
pub fn enabled_order(db: &crate::storage::Db) -> Vec<String> {
    db.with(|c| {
        let mut statement = c.prepare("SELECT id FROM packages WHERE enabled = 1 ORDER BY updated_at DESC, id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    })
    .unwrap_or_default()
}

/// A package removed: its row goes with it.
pub fn forget(db: &crate::storage::Db, id: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("DELETE FROM packages WHERE id = ?1", params![id]).map(|_| ()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A package written into `root/<id>/<version>/`.
    pub fn write_package(root: &Path, manifest: &Value, code: Option<&str>) -> PathBuf {
        let id = manifest["id"].as_str().unwrap();
        let version = manifest["version"].as_str().unwrap();
        let dir = root.join(id).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(manifest).unwrap()).unwrap();
        if let Some(code) = code {
            std::fs::write(dir.join("backend.js"), code).unwrap();
        }
        dir
    }

    pub fn manifest(id: &str, version: &str) -> Value {
        json!({
            "schema": 1, "id": id, "version": version, "title": id, "api": "^2", "runs": ["pc", "station", "stationServer"],
            "permissions": [],
            "contributes": { "tools": [{ "name": format!("{}_echo", id.replace('-', "_")), "description": "Повторяет" }] }
        })
    }

    #[test]
    fn finds_each_plugins_versions_newest_first_and_tells_what_is_wrong_with_a_broken_one() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &manifest("radio", "1.2.0"), Some("export const x = 1;"));
        write_package(dir.path(), &manifest("radio", "1.10.0"), Some("export const x = 1;"));
        write_package(dir.path(), &manifest("radio", "0.9.0"), None);
        let weather = write_package(dir.path(), &manifest("weather", "1.0.0"), Some(""));
        let mut wrong = manifest("weather", "1.0.0");
        wrong["id"] = json!("other");
        std::fs::write(weather.join("manifest.json"), wrong.to_string()).unwrap();
        std::fs::create_dir_all(dir.path().join("Bad Name/1.0.0")).unwrap();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let found = scan(dir.path());
        assert_eq!(found.keys().collect::<Vec<_>>(), ["radio", "weather"]);
        let versions: Vec<String> = found["radio"].iter().map(Candidate::version).collect();
        assert_eq!(versions, ["1.10.0", "1.2.0", "0.9.0"]);
        assert!(
            matches!(&found["radio"][2], Candidate::Broken { problems, .. } if problems[0].starts_with("нужен backend.js"))
        );
        assert!(
            matches!(&found["weather"][0], Candidate::Broken { problems, .. } if problems[0].contains("не совпадает с папкой"))
        );
        assert!(scan(&dir.path().join("nothing")).is_empty());
    }

    #[test]
    fn keeps_the_admins_choice_and_the_version_that_started() {
        let db = crate::storage::Db::memory();
        assert!(choices(&db).is_empty());
        set_enabled(&db, "radio", true, Some("1.0.0"), Some("Админ")).unwrap();
        set_good(&db, "radio", "1.1.0").unwrap();
        set_enabled(&db, "radio", false, None, None).unwrap();
        let local = Choice {
            enabled: false,
            good: Some("1.1.0".into()),
            source: SOURCE_LOCAL.into(),
            verified: false,
            feed: None,
            hidden: false,
        };
        assert_eq!(choices(&db)["radio"], local);
        // The installer notes where it came from; on or off and the good version stay.
        set_installed(&db, "radio", SOURCE_CATALOG, true, Some("Админ")).unwrap();
        let from_catalog = Choice { source: SOURCE_CATALOG.into(), verified: true, ..local };
        assert_eq!(choices(&db)["radio"], from_catalog);
        set_installed(&db, "arthur", SOURCE_CATALOG, true, None).unwrap();
        assert!(!choices(&db)["arthur"].enabled, "a new package is off");
        // Its feed, and that feed hiding it: told once; a package put in anew is shown and from no feed.
        set_feed(&db, "arthur", "https://feed.example/catalog.json").unwrap();
        assert!(set_hidden(&db, "arthur", true).unwrap());
        assert!(!set_hidden(&db, "arthur", true).unwrap(), "no change");
        let hidden = &choices(&db)["arthur"];
        assert_eq!((hidden.feed.as_deref(), hidden.hidden), (Some("https://feed.example/catalog.json"), true));
        set_installed(&db, "arthur", SOURCE_LOCAL, false, None).unwrap();
        assert_eq!((choices(&db)["arthur"].feed.clone(), choices(&db)["arthur"].hidden), (None, false));
        forget(&db, "radio").unwrap();
        assert!(!choices(&db).contains_key("radio"));
    }
}
