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
pub const MAX_SOURCES: usize = 4;
/// The own sources' file in the catalog's folder (plugins/catalog.rs).
pub const SOURCES_FILE: &str = "sources.json";

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
