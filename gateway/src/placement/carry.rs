//! What a move to the station carries besides the settings (clean-path check 18.11, item 43): the computer's own
//! sources of packages (`packages-catalog/sources.json`, at most four) and its installed characters — each packed
//! from its folder here into the archive the catalog would have given (a `.vgp`: tar.gz with `manifest.json` at its
//! root), with its SHA-256. A character chosen before the station was set up (an invitation's demo, first steps) then
//! stands at the centre as here: its package, the source it came from, the choice (that travels with the settings).
//!
//! The station is tight (/data ~14 MB free): the computer counts each package unpacked and carries only what fits the
//! room left after the program, its data and the floor no update takes — the chosen character first. What stays
//! behind is said in the move's notes, never lost quietly. The station's gateway installs them at its first start,
//! before any service reads the packages (`take`): the hash checked, the registry row as here (source, verified, feed),
//! a package it has already left as it is.
use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plugins::install::{install_archive, Expected};
use crate::plugins::manifest::PackageKind;
use crate::plugins::registry;
use crate::storage::Db;

/// Own sources, as many as the catalog keeps (plugins/catalog.rs `MAX_SOURCES`).
const MAX_SOURCES: usize = 4;
/// The own sources' file in the catalog's folder (plugins/catalog.rs).
const SOURCES_FILE: &str = "sources.json";

/// What travels with a move to the station besides the settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Carried {
    /// The own sources as `sources.json` keeps them (`{url, addedAt}`).
    #[serde(default)]
    pub sources: Vec<Value>,
    #[serde(default)]
    pub packages: Vec<CarriedPackage>,
}

/// One installed package on its way: the archive packed from its folder, in base64, and its registry row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarriedPackage {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub archive: String,
    pub source: String,
    pub verified: bool,
    #[serde(default)]
    pub feed: Option<String>,
}

/// What the computer gathered, and the characters that did not fit — by title, for the move's note.
#[cfg(feature = "pc")]
pub struct Gathered {
    pub carried: Carried,
    pub left: Vec<(String, u64)>,
}

/// The own sources and the installed characters here, the `chosen` package first, as many as fit `room_kb` unpacked
/// (None: all). A hidden one (its feed said so) stays: it is not offered here either.
#[cfg(feature = "pc")]
pub fn gather(
    packages_root: &Path,
    catalog_dir: &Path,
    db: &Db,
    chosen: Option<&str>,
    room_kb: Option<u64>,
) -> Gathered {
    let sources = std::fs::read_to_string(catalog_dir.join(SOURCES_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
        .unwrap_or_default();
    let choices = registry::choices(db);
    let mut characters: Vec<_> = registry::scan(packages_root)
        .into_values()
        .filter_map(|found| registry::newest_ready(&found))
        .filter(|package| package.manifest.kind == PackageKind::Character)
        .filter(|package| !choices.get(&package.manifest.id).is_some_and(|choice| choice.hidden))
        .collect();
    characters.sort_by_key(|package| Some(package.manifest.id.as_str()) != chosen);
    let mut room = room_kb;
    let (mut packages, mut left) = (Vec::new(), Vec::new());
    for package in characters {
        let id = package.manifest.id.clone();
        let kb = folder_size(&package.dir).div_ceil(1024);
        if room.is_some_and(|room| kb > room) {
            left.push((package.manifest.title.clone(), kb));
            continue;
        }
        let bytes = match pack(&package.dir) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(target: "placement", "{id}: not packed for the station: {error}");
                left.push((package.manifest.title.clone(), kb));
                continue;
            }
        };
        room = room.map(|room| room - kb);
        let choice = choices.get(&id).cloned().unwrap_or_default();
        packages.push(CarriedPackage {
            sha256: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)),
            archive: B64.encode(&bytes),
            version: package.version(),
            id,
            source: choice.source,
            verified: choice.verified,
            feed: choice.feed,
        });
    }
    Gathered { carried: Carried { sources: sources.into_iter().take(MAX_SOURCES).collect(), packages }, left }
}

/// The files of a folder, in bytes, all the way down.
pub fn folder_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// A package's folder as the archive it came in: tar.gz, its files and folders by their paths inside it.
#[cfg(feature = "pc")]
fn pack(dir: &Path) -> std::io::Result<Vec<u8>> {
    fn add(builder: &mut tar::Builder<impl std::io::Write>, root: &Path, dir: &Path) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let inside = path.strip_prefix(root).map_err(std::io::Error::other)?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                builder.append_dir(inside, &path)?;
                add(builder, root, &path)?;
            } else if kind.is_file() {
                builder.append_path_with_name(&path, inside)?;
            }
        }
        Ok(())
    }
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    builder.follow_symlinks(false);
    add(&mut builder, dir, dir)?;
    builder.into_inner()?.finish()
}

/// At the station's start, before the services: the own sources joined to its own (at most four) and each carried
/// package installed, its hash checked; one already here at that version is left as it is. Whatever fails is said in
/// the journal and the rest goes on. How many packages it installed.
pub fn take(carried: &Carried, packages_root: &Path, catalog_dir: &Path, db: &Db) -> usize {
    if !carried.sources.is_empty() {
        let file = catalog_dir.join(SOURCES_FILE);
        let mut sources: Vec<Value> =
            std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        let url = |source: &Value| source.get("url").and_then(Value::as_str).map(str::to_string);
        for source in &carried.sources {
            if sources.len() < MAX_SOURCES
                && url(source).is_some()
                && !sources.iter().any(|kept| url(kept) == url(source))
            {
                sources.push(source.clone());
            }
        }
        let written = std::fs::create_dir_all(catalog_dir)
            .and_then(|_| std::fs::write(&file, serde_json::to_vec_pretty(&sources).unwrap_or_default()));
        match written {
            Ok(()) => tracing::info!(target: "placement", "own sources from the computer: {} kept", sources.len()),
            Err(error) => tracing::error!(target: "placement", "own sources from the computer not kept: {error}"),
        }
    }
    let mut installed = 0;
    for package in &carried.packages {
        if packages_root.join(&package.id).join(&package.version).join("manifest.json").is_file() {
            continue;
        }
        let bytes = match B64.decode(&package.archive) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(target: "placement", "{}: the computer's archive does not read: {error}", package.id);
                continue;
            }
        };
        let expected =
            Expected { id: &package.id, version: &package.version, sha256: &package.sha256, size: bytes.len() as u64 };
        let placed = install_archive(packages_root, &bytes, expected).and_then(|placed| {
            if placed.kind == PackageKind::Character {
                Ok(placed)
            } else {
                let _ = std::fs::remove_dir_all(&placed.dir);
                Err("not a character".into())
            }
        });
        match placed {
            Ok(_) => {
                let row = registry::set_installed(db, &package.id, &package.source, package.verified, Some("перенос"))
                    .and_then(|_| {
                        package.feed.as_deref().map_or(Ok(()), |feed| registry::set_feed(db, &package.id, feed))
                    });
                if let Err(error) = row {
                    tracing::error!(target: "placement", "{}: installed, its registry row not written: {error}", package.id);
                }
                installed += 1;
                tracing::info!(target: "placement", "{} {}: installed from the computer's archive", package.id, package.version);
            }
            Err(error) => {
                tracing::error!(target: "placement", "{}: not installed from the computer: {error}", package.id)
            }
        }
    }
    installed
}
