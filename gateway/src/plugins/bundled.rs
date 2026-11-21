//! Packages built into the gateway (docs/plans/plugins.md, «Экран «Плагины» и три вида пакетов»), like the panel: the
//! folders of `packages/` that `packages/bundled.json` names, embedded at build time and laid out into PACKAGES_DIR at
//! every start, each marked with its kind. `bundled.json` is keyed by the package's id — a random one, like every
//! package's (docs/plans/random-package-ids.md) — and names its `folder` in `packages/`, which may stay readable:
//!
//! - **system** — without it the station does not work as it should: never switched off, never removed;
//! - **bundled** — «Радио» and the built-ins as they move into plugins (stage 10): switched off at will, never
//!   removed — it is updated with the gateway, and «Сделать свою копию» gives one to change.
//!
//! The kind is the gateway's to set, by where a package came from, never the manifest's: a plugin cannot declare
//! itself a system one. A new version of the gateway brings a new version beside the old one (which stays, the one
//! to fall back on); older ones go. A package seen for the first time is switched on or off as `bundled.json` says;
//! after that the owner's choice stays. A built-in an earlier build laid out and this one no longer has — dropped from
//! the build, or renamed (`radio` → `p-a2fd1d7eb8a4`, 20.11.2026) — goes whole (`retire`): no twin of a built-in stays
//! that the panel cannot remove.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::params;
use serde_json::Value;

use super::manifest;
use super::registry::{self, SOURCE_BUNDLED, SOURCE_SYSTEM};
use crate::storage::Db;

#[derive(rust_embed::RustEmbed)]
#[folder = "../packages/"]
struct Embedded;

/// How many versions of a built-in package stay on disk: the one in force and the one before it.
const KEPT: usize = 2;

/// A package of the gateway's own build.
#[derive(Debug, Clone)]
pub struct BundledPackage {
    pub id: String,
    pub version: String,
    /// `system` or `bundled`: the source the registry keeps.
    pub source: &'static str,
    /// Whether it is on the first time it is seen.
    pub enabled: bool,
    /// Its files by path inside the package.
    pub files: Vec<(String, Cow<'static, [u8]>)>,
}

/// The packages built into this gateway, as `packages/bundled.json` names them; one that does not read is left out
/// with a warning (the build's tests keep that from happening).
pub fn packages() -> Vec<BundledPackage> {
    let Some(index) = Embedded::get("bundled.json") else { return Vec::new() };
    let index: Value = match serde_json::from_slice(&index.data) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(target: "plugins", "packages/bundled.json does not read: {error}");
            return Vec::new();
        }
    };
    // Each package's files by its id: the path inside the package and the bytes.
    type Files = Vec<(String, Cow<'static, [u8]>)>;
    let mut files: BTreeMap<String, Files> = BTreeMap::new();
    for path in Embedded::iter() {
        let Some((id, inside)) = path.split_once('/') else { continue };
        if let Some(file) = Embedded::get(&path) {
            files.entry(id.to_string()).or_default().push((inside.to_string(), file.data));
        }
    }
    let mut found = Vec::new();
    for (id, entry) in index.as_object().into_iter().flatten() {
        if id.starts_with('_') {
            continue;
        }
        let source = match entry.get("origin").and_then(Value::as_str) {
            Some("system") => SOURCE_SYSTEM,
            Some("bundled") => SOURCE_BUNDLED,
            other => {
                tracing::warn!(target: "plugins", "packages/bundled.json: {id}: origin {other:?} is neither system nor bundled");
                continue;
            }
        };
        // The folder in packages/: readable (`radio`), while the id is random; without one, the id itself.
        let folder = entry.get("folder").and_then(Value::as_str).unwrap_or(id);
        let Some(package_files) = files.remove(folder) else {
            tracing::warn!(target: "plugins", "packages/bundled.json: {id}: no folder {folder} in packages/");
            continue;
        };
        match read(id, source, entry.get("enabled").and_then(Value::as_bool).unwrap_or(false), package_files) {
            Ok(package) => found.push(package),
            Err(problem) => tracing::warn!(target: "plugins", "built-in package {id}: {problem}"),
        }
    }
    found
}

/// A built-in package from its files: its manifest must read and name it.
pub fn read(
    id: &str,
    source: &'static str,
    enabled: bool,
    files: Vec<(String, Cow<'static, [u8]>)>,
) -> Result<BundledPackage, String> {
    let manifest =
        files.iter().find(|(path, _)| path == "manifest.json").ok_or_else(|| "no manifest.json".to_string())?;
    let manifest = manifest::parse(&String::from_utf8_lossy(&manifest.1)).map_err(|error| error.to_string())?;
    if manifest.id != id {
        return Err(format!("its manifest says id «{}»", manifest.id));
    }
    Ok(BundledPackage { id: id.to_string(), version: manifest.version.to_string(), source, enabled, files })
}

/// The ids `bundled.json` names, read or not: what `retire` never takes away. Empty when it does not read.
pub fn named() -> Vec<String> {
    let Some(index) = Embedded::get("bundled.json") else { return Vec::new() };
    let Ok(index) = serde_json::from_slice::<Value>(&index.data) else { return Vec::new() };
    index.as_object().into_iter().flatten().map(|(id, _)| id).filter(|id| !id.starts_with('_')).cloned().collect()
}

/// The gateway's own packages (`bundled`, `system`) an earlier build laid out that are not among `kept` — a built-in
/// dropped from the build, or one renamed (the random ids of 20.11.2026, docs/plans/random-package-ids.md): each goes
/// whole — its folder in `root`, its row, its `kv`, its rules and its long tasks — and its id is returned for the
/// rest (keys, settings) to go too. A copy of one's own (`local`), the catalog's and what was put in by hand stay.
/// Nothing goes while `kept` is empty: a build without its packages (`PLUGINS_BUNDLED=0`, a `bundled.json` that does
/// not read) takes nobody's away.
pub fn retire(root: &Path, db: &Db, kept: &BTreeSet<String>) -> Vec<String> {
    if kept.is_empty() {
        return Vec::new();
    }
    let mut stale: Vec<String> = registry::choices(db)
        .into_iter()
        .filter(|(id, choice)| {
            (choice.source == SOURCE_BUNDLED || choice.source == SOURCE_SYSTEM)
                && !kept.contains(id)
                && manifest::valid_id(id)
        })
        .map(|(id, _)| id)
        .collect();
    stale.sort();
    let mut gone = Vec::new();
    for id in stale {
        let dir = root.join(&id);
        if dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(target: "plugins", "built-in package {id} of an earlier build: its folder stays: {error}");
                continue;
            }
        }
        let forgotten = db.with(|c| {
            c.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1", params![id])?;
            c.execute("DELETE FROM app_settings WHERE app_id = ?1", params![id])?;
            c.execute("DELETE FROM app_tasks WHERE app = ?1", params![id])?;
            c.execute("DELETE FROM packages WHERE id = ?1", params![id]).map(|_| ())
        });
        match forgotten {
            Ok(()) => {
                tracing::info!(target: "plugins", "built-in package {id} of an earlier build removed: this build has none such");
                gone.push(id);
            }
            Err(error) => {
                tracing::warn!(target: "plugins", "built-in package {id} of an earlier build: not forgotten: {error}")
            }
        }
    }
    gone
}

/// Lays the built-in packages out in `root`: a version not there yet is written (beside, then renamed into place),
/// the registry notes its kind, a package seen for the first time is switched on or off as the build says, versions
/// older than the one before go. Whatever fails is told in the log; the gateway starts anyway.
pub fn lay_out(root: &Path, db: &Db, packages: &[BundledPackage]) {
    let choices = registry::choices(db);
    for package in packages {
        let dir = root.join(&package.id).join(&package.version);
        if !dir.join("manifest.json").is_file() {
            match write(root, package) {
                Ok(()) => {
                    tracing::info!(target: "plugins", "built-in package {} {} laid out", package.id, package.version)
                }
                Err(error) => {
                    tracing::warn!(target: "plugins", "built-in package {} {}: not laid out: {error}", package.id, package.version);
                    continue;
                }
            }
        }
        let known = choices.get(&package.id);
        if known.is_none_or(|choice| choice.source != package.source) {
            let first = known.is_none();
            let marked = registry::set_installed(db, &package.id, package.source, true, None).and_then(|()| {
                if first && package.enabled {
                    registry::set_enabled(db, &package.id, true, None, None)
                } else {
                    Ok(())
                }
            });
            if let Err(error) = marked {
                tracing::warn!(target: "plugins", "built-in package {}: not noted: {error}", package.id);
            }
        }
        prune(root, &package.id, &package.version);
    }
}

fn write(root: &Path, package: &BundledPackage) -> std::io::Result<()> {
    let partial = root.join(format!(".partial-{}-{}", package.id, package.version));
    let _ = std::fs::remove_dir_all(&partial);
    for (path, bytes) in &package.files {
        // Only plain paths inside the package: the build's own files, but the rule is the installer's too.
        if path.split('/').any(|part| part.is_empty() || part == "." || part == ".." || part.starts_with('.')) {
            continue;
        }
        let file = partial.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, bytes)?;
    }
    let target = root.join(&package.id).join(&package.version);
    std::fs::create_dir_all(root.join(&package.id))?;
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&partial, &target)
}

/// Versions of a built-in package older than the one before `current` go.
fn prune(root: &Path, id: &str, current: &str) {
    let Ok(current) = semver::Version::parse(current) else { return };
    let mut versions: Vec<semver::Version> = std::fs::read_dir(root.join(id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| semver::Version::parse(&name).ok())
        .filter(|version| *version <= current)
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    for old in versions.into_iter().skip(KEPT) {
        let _ = std::fs::remove_dir_all(root.join(id).join(old.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn package(id: &str, version: &str, source: &'static str, enabled: bool) -> BundledPackage {
        let manifest = json!({
            "schema": 1, "id": id, "version": version, "title": id, "api": "^2", "runs": ["pc", "station", "stationServer"],
            "permissions": [],
            "contributes": { "intents": [{ "phrases": [format!("скажи {id}")], "handler": "say" }] }
        });
        let files = vec![
            ("manifest.json".to_string(), Cow::Owned(manifest.to_string().into_bytes())),
            ("backend.js".to_string(), Cow::Borrowed("export function say() { return 'да'; }".as_bytes())),
            ("assets/a.txt".to_string(), Cow::Borrowed(&b"a"[..])),
        ];
        read(id, source, enabled, files).unwrap()
    }

    #[test]
    fn the_builds_own_packages_read_and_radio_is_among_them() {
        let built = packages();
        // Every id is a random one (docs/plans/random-package-ids.md); «Радио» lies in packages/radio.
        let ids: Vec<&str> = built.iter().map(|package| package.id.as_str()).collect();
        assert!(ids.iter().all(|id| manifest::random_id(id)), "{ids:?}");
        let radio = built.iter().find(|package| package.id == "p-a2fd1d7eb8a4").expect("radio is built in");
        assert_eq!((radio.source, radio.enabled), (SOURCE_BUNDLED, true));
        assert!(radio.files.iter().any(|(path, _)| path == "backend.js"));
    }

    /// The built-in apps' tools go to the model with every question: English and short, as ours
    /// (docs/plugins/AUTHORING.md, `tools`; docs/plans/fast-tools.md).
    #[test]
    fn the_built_in_apps_tools_speak_english() {
        let mut seen = 0;
        for package in packages() {
            let (_, manifest) = package.files.iter().find(|(path, _)| path == "manifest.json").expect("a manifest");
            let manifest: Value = serde_json::from_slice(manifest).unwrap();
            for tool in manifest["contributes"]["tools"].as_array().into_iter().flatten() {
                seen += 1;
                let found = crate::skills::tools::not_in_english(tool);
                assert!(found.is_empty(), "{} {}: {found:?}", package.id, tool["name"]);
            }
        }
        assert!(seen >= 14, "{seen} tools");
    }

    #[test]
    fn lays_out_marks_switches_on_the_first_time_only_and_keeps_two_versions() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory();
        lay_out(
            dir.path(),
            &db,
            &[package("radio", "1.0.0", SOURCE_BUNDLED, true), package("core", "1.0.0", SOURCE_SYSTEM, false)],
        );
        assert!(dir.path().join("radio/1.0.0/assets/a.txt").is_file());
        let choices = registry::choices(&db);
        assert_eq!((choices["radio"].source.as_str(), choices["radio"].enabled), (SOURCE_BUNDLED, true));
        assert_eq!((choices["core"].source.as_str(), choices["core"].enabled), (SOURCE_SYSTEM, false));
        // The owner switched it off: a new build does not switch it on again.
        registry::set_enabled(&db, "radio", false, None, None).unwrap();
        lay_out(dir.path(), &db, &[package("radio", "1.1.0", SOURCE_BUNDLED, true)]);
        lay_out(dir.path(), &db, &[package("radio", "1.2.0", SOURCE_BUNDLED, true)]);
        assert!(!registry::choices(&db)["radio"].enabled);
        let mut kept: Vec<String> = std::fs::read_dir(dir.path().join("radio"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        kept.sort();
        assert_eq!(kept, ["1.1.0", "1.2.0"], "the one in force and the one before");
        // A package copied in by hand with a built-in's id becomes the built-in one.
        registry::set_installed(&db, "radio", registry::SOURCE_LOCAL, false, None).unwrap();
        lay_out(dir.path(), &db, &[package("radio", "1.2.0", SOURCE_BUNDLED, true)]);
        assert_eq!(registry::choices(&db)["radio"].source, SOURCE_BUNDLED);
    }

    /// A built-in of an earlier build this one has not — renamed (`radio` → a random id) or dropped — goes whole; a
    /// copy of one's own, the catalog's and the new built-in stay; a build without its packages takes nothing away.
    #[test]
    fn retires_the_built_ins_an_earlier_build_laid_out_and_keeps_everyone_elses() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory();
        // 0.16: «Радио» as `radio`, a system one, and a dropped built-in; a copy of one's own and a catalog's beside.
        lay_out(
            dir.path(),
            &db,
            &[
                package("radio", "1.0.0", SOURCE_BUNDLED, true),
                package("core", "1.0.0", SOURCE_SYSTEM, true),
                package("dropped", "1.0.0", SOURCE_BUNDLED, false),
            ],
        );
        for (id, source) in [("my-radio", registry::SOURCE_LOCAL), ("arthur", registry::SOURCE_CATALOG)] {
            std::fs::create_dir_all(dir.path().join(id).join("1.0.0")).unwrap();
            registry::set_installed(&db, id, source, false, None).unwrap();
        }
        db.with(|c| {
            for id in ["radio", "my-radio"] {
                c.execute(
                    "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, 'count', '3', 'now')",
                    params![id],
                )?;
                c.execute(
                    "INSERT INTO app_settings (app_id, key, value, updated_at) VALUES (?1, 'rule', '1', 'now')",
                    params![id],
                )?;
            }
            c.execute(
                "INSERT INTO app_tasks (id, app, title, state, started_ms, updated_ms) VALUES ('t1', 'radio', 'x', 'done', 1, 1)",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
        let rows = |table: &str, column: &str, id: &str| -> i64 {
            db.with(|c| {
                c.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"), params![id], |row| row.get(0))
            })
            .unwrap()
        };

        // A build without its packages takes nobody's away.
        assert!(retire(dir.path(), &db, &BTreeSet::new()).is_empty());
        assert!(dir.path().join("radio").is_dir());

        // 0.17: «Радио» is `p-a2fd1d7eb8a4`, the system one stays in the build, `dropped` is gone from it.
        let new = package("p-a2fd1d7eb8a4", "1.0.0", SOURCE_BUNDLED, true);
        lay_out(dir.path(), &db, &[new, package("core", "1.0.0", SOURCE_SYSTEM, true)]);
        let kept: BTreeSet<String> = ["p-a2fd1d7eb8a4", "core"].map(String::from).into();
        assert_eq!(retire(dir.path(), &db, &kept), ["dropped", "radio"]);
        let choices = registry::choices(&db);
        for id in ["radio", "dropped"] {
            assert!(!dir.path().join(id).exists() && !choices.contains_key(id), "{id} goes whole");
        }
        assert_eq!(
            (
                rows("plugin_kv", "plugin_id", "radio"),
                rows("app_settings", "app_id", "radio"),
                rows("app_tasks", "app", "radio")
            ),
            (0, 0, 0)
        );
        for id in ["p-a2fd1d7eb8a4", "core", "my-radio", "arthur"] {
            assert!(dir.path().join(id).is_dir() && choices.contains_key(id), "{id} stays");
        }
        assert_eq!((rows("plugin_kv", "plugin_id", "my-radio"), rows("app_settings", "app_id", "my-radio")), (1, 1));
        assert!(retire(dir.path(), &db, &kept).is_empty(), "once");
    }

    /// Host API 2: the built-in apps' widgets are cards the panel draws — no code of theirs runs in the panel — and
    /// each card's function is among their panel's calls (docs/plans/bus.md, «Один способ»). A game is the one
    /// exception: its own drawing, keys and sound are a window's, in its frame like any app's («Неоновая змейка»).
    #[test]
    fn every_built_in_widget_is_a_card_and_no_built_in_brings_panel_code() {
        const WINDOWS: &[&str] = &["p-6d9f10977863"]; // «Неоновая змейка»
        let mut cards = 0;
        for package in packages() {
            assert_eq!(
                package.files.iter().any(|(path, _)| path == "panel.js" || path == "panel.css"),
                WINDOWS.contains(&package.id.as_str()),
                "{}: panel code only for a game",
                package.id
            );
            let manifest = package.files.iter().find(|(path, _)| path == "manifest.json").unwrap();
            let manifest: Value = serde_json::from_slice(&manifest.1).unwrap();
            let panel = &manifest["contributes"]["panel"];
            for card in panel["cards"].as_array().into_iter().flatten() {
                let calls = panel["calls"].as_array().unwrap();
                assert!(calls.contains(&card["function"]), "{}: {card}", package.id);
                cards += 1;
            }
        }
        assert!(cards >= 5, "weather, currency, radio, shopping, calendar: {cards}");
    }

    #[tokio::test]
    async fn every_built_in_package_reads_loads_and_exports_its_functions() {
        let index: Value = serde_json::from_slice(&Embedded::get("bundled.json").unwrap().data).unwrap();
        let named: Vec<&String> = index.as_object().unwrap().keys().filter(|id| !id.starts_with('_')).collect();
        let built = packages();
        assert_eq!(built.len(), named.len(), "every package bundled.json names is built in");
        let dir = tempfile::tempdir().unwrap();
        for package in &built {
            let folder = dir.path().join(&package.id);
            for (path, bytes) in &package.files {
                let file = folder.join(path);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, bytes).unwrap();
            }
            let report = crate::plugins::workshop::sandbox::check(&folder, &package.id, None).await;
            assert!(report.ok, "{}: {report:?}", package.id);
        }
    }
}
