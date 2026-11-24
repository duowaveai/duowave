//! What a move to the station carries — the computer's part, added to «Дом»'s `placement::carry`: the own sources and
//! the installed characters gathered here, each packed from its folder into the archive the catalog would have given.

use std::path::Path;

use base64::Engine as _;
use serde_json::Value;

pub use duowave_home::placement::carry::*;

use crate::plugins::manifest::PackageKind;
use crate::plugins::registry;
use crate::storage::Db;
use base64::engine::general_purpose::STANDARD as B64;

/// What the computer gathered, and the characters that did not fit — by title, for the move's note.
pub struct Gathered {
    pub carried: Carried,
    pub left: Vec<(String, u64)>,
}

/// The own sources and the installed characters here, the `chosen` package first, as many as fit `room_kb` unpacked
/// (None: all). A hidden one (its feed said so) stays: it is not offered here either.
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

/// A package's folder as the archive it came in: tar.gz, its files and folders by their paths inside it.
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
