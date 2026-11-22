//! Installing a package: one way in for every kind (docs/plans/characters.md, section 4). The archive (`.vgp`: a
//! tar.gz with `manifest.json` and `assets/` at its root) is checked against the catalog's size and SHA-256 before
//! anything of it is unpacked; it is unpacked into `PACKAGES_DIR/.partial-<id>-<version>` with paths only inside that
//! folder, regular files and folders only, within the limits below; its manifest must read as a package and name the
//! id and version expected; then the folder is renamed into `PACKAGES_DIR/<id>/<version>/`. Whatever fails on the
//! way, nothing of it is left behind: the registry never sees half a package. Removing is the folder of every version.
//!
//! An update is unpacked beside the version installed (`beside`): a file the same as that version's at the same path
//! — same size, same bytes — is a hard link to it, not written again, so the disk takes only what changed. On the
//! station that is what lets an app of 14 MB on a /data of 15 MB update when only its manifest changed (the
//! clean-path check, 18.11).
use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::manifest::{valid_id, PackageKind};
use super::registry::{self, Candidate};

/// An archive bigger than this is not one of ours.
pub const MAX_ARCHIVE: u64 = 16 * 1024 * 1024;
/// What it may unpack into, at most.
pub const MAX_UNPACKED: u64 = 32 * 1024 * 1024;
/// How many files it may hold.
pub const MAX_FILES: usize = 500;
/// Files a browser would run on the panel's origin: none in a package.
const REFUSED: &[&str] = &["svg", "svgz"];

/// What the catalog promised of the archive.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub id: &'a str,
    pub version: &'a str,
    /// Lower-case hex.
    pub sha256: &'a str,
    pub size: u64,
}

/// A package put in place.
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub kind: PackageKind,
    pub dir: PathBuf,
}

/// The archive is what the catalog said: its size, then its hash.
pub fn check_archive(bytes: &[u8], sha256: &str, size: u64) -> Result<(), String> {
    if bytes.len() as u64 > MAX_ARCHIVE {
        return Err(crate::tr!("архив больше {max} МБ", max = MAX_ARCHIVE / 1024 / 1024));
    }
    if bytes.len() as u64 != size {
        return Err(crate::tr!("размер архива {got} байт, а в каталоге — {size}", got = bytes.len(), size = size));
    }
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(sha256) {
        return Err(crate::tr!("хеш архива не совпал с каталогом: архив подменён или испорчен"));
    }
    Ok(())
}

/// A path of the archive as a path inside the package: relative, no `..`, no root; `./` parts dropped. None: the
/// archive's own root.
pub(crate) fn inside(path: &Path) -> Result<Option<PathBuf>, String> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(crate::tr!("путь «{path}» ведёт из папки приложения", path = path.display()));
            }
        }
    }
    Ok((!clean.as_os_str().is_empty()).then_some(clean))
}

/// What an archive's files are beside the version installed: what of them the disk would take, and which of them
/// that version has already.
#[derive(Debug, Clone, Default)]
pub struct Beside {
    /// The version installed, its folder: none for a first install.
    pub dir: Option<PathBuf>,
    /// The bytes of the files that are new or not the same as the installed version's: what unpacking writes.
    pub changed_bytes: u64,
    /// The files (paths inside the package) the same as the installed version's at the same path, size and bytes:
    /// hard-linked to them, they take nothing of the disk.
    pub same: BTreeSet<PathBuf>,
}

impl Beside {
    /// What unpacking takes of the disk, in MB rounded up.
    pub fn changed_mb(&self) -> u64 {
        self.changed_bytes.div_ceil(1024 * 1024)
    }

    /// The installed version's file to link for this one, when it is the same and still there as it was compared.
    fn link_for(&self, relative: &Path, size: u64) -> Option<PathBuf> {
        let old = self.dir.as_ref()?.join(relative);
        let meta = std::fs::symlink_metadata(&old).ok()?;
        (self.same.contains(relative) && meta.is_file() && meta.len() == size).then_some(old)
    }
}

/// Reads `entry` to its end, at most `limit` bytes and one more: how many bytes it held, and whether they are those
/// of the regular file `old` byte for byte (false when there is no such file).
fn read_against(entry: &mut impl std::io::Read, old: Option<&Path>, limit: u64) -> std::io::Result<(u64, bool)> {
    let mut old = old
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
        .and_then(|path| std::fs::File::open(path).ok());
    let mut same = old.is_some();
    let (mut chunk, mut theirs) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    let mut limited = entry.take(limit + 1);
    let mut count = 0u64;
    loop {
        let read = limited.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if let (true, Some(file)) = (same, old.as_mut()) {
            same = file.read_exact(&mut theirs[..read]).is_ok() && theirs[..read] == chunk[..read];
        }
    }
    // The old file ends where the entry does.
    if let (true, Some(file)) = (same, old.as_mut()) {
        same = file.read(&mut theirs[..1])? == 0;
    }
    Ok((count, same))
}

/// The archive's files beside the version installed in `previous` (none: a first install): the bytes unpacking
/// would write and the files it would link instead — compared byte for byte as they are read, nothing written. The
/// archive is in memory already; this is what the room on the disk is counted by, before anything is unpacked.
/// Within the same limits as `unpack`; what `unpack` refuses besides is left to it.
pub fn beside(bytes: &[u8], previous: Option<&Path>) -> Result<Beside, String> {
    let broken = |error: std::io::Error| crate::tr!("архив не читается: {error}", error = error);
    let mut result = Beside { dir: previous.map(Path::to_path_buf), ..Beside::default() };
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let (mut entries, mut total) = (0usize, 0u64);
    for entry in archive.entries().map_err(broken)? {
        let mut entry = entry.map_err(broken)?;
        let kind = entry.header().entry_type();
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        entries += 1;
        if entries > MAX_FILES * 2 {
            return Err(crate::tr!("в архиве больше {max} файлов", max = MAX_FILES));
        }
        if !matches!(kind, tar::EntryType::Regular | tar::EntryType::Continuous) {
            continue;
        }
        let path = entry.path().map_err(broken)?.into_owned();
        let Some(relative) = inside(&path)? else { continue };
        let old = previous.map(|dir| dir.join(&relative));
        // A file of another size is not the same: not even read against it.
        let old = old.filter(|old| std::fs::metadata(old).is_ok_and(|meta| meta.len() == entry.size()));
        let (read, same) = read_against(&mut entry, old.as_deref(), MAX_UNPACKED - total).map_err(broken)?;
        total += read;
        if total > MAX_UNPACKED {
            return Err(crate::tr!("распакованное приложение больше {max} МБ", max = MAX_UNPACKED / 1024 / 1024));
        }
        if same {
            result.same.insert(relative);
        } else {
            result.changed_bytes += read;
        }
    }
    Ok(result)
}

/// Unpacks a tar.gz into `into` (made here): regular files and folders only, inside it, within the limits. Beside
/// the version installed (`previous`, counted by `beside`), a file the same as its own is a hard link to it — the
/// same filesystem, both under PACKAGES_DIR — and written as any other when the link cannot be made. Whatever reads
/// the files afterwards (the hash of an app's build, `apps::supervisor::checked_build`) reads the same bytes.
pub fn unpack(bytes: &[u8], into: &Path, previous: Option<&Beside>) -> Result<(), String> {
    let broken = |error: std::io::Error| crate::tr!("архив не читается: {error}", error = error);
    std::fs::create_dir_all(into).map_err(|error| format!("{}: {error}", into.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let (mut files, mut entries, mut total) = (0usize, 0usize, 0u64);
    for entry in archive.entries().map_err(broken)? {
        let mut entry = entry.map_err(broken)?;
        let kind = entry.header().entry_type();
        // A pax header of the whole archive (git archive writes one): nothing to unpack.
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        entries += 1;
        if entries > MAX_FILES * 2 {
            return Err(crate::tr!("в архиве больше {max} файлов", max = MAX_FILES));
        }
        let path = entry.path().map_err(broken)?.into_owned();
        let Some(relative) = inside(&path)? else { continue };
        let target = into.join(&relative);
        match kind {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                files += 1;
                if files > MAX_FILES {
                    return Err(crate::tr!("в архиве больше {max} файлов", max = MAX_FILES));
                }
                let extension = relative.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if REFUSED.contains(&extension.as_str()) {
                    return Err(crate::tr!("«{path}»: SVG в приложении нельзя", path = relative.display()));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                // The same as the installed version's: linked, its bytes left unread (the archive skips them).
                if let Some(old) = previous.and_then(|previous| previous.link_for(&relative, entry.size())) {
                    match std::fs::hard_link(&old, &target) {
                        Ok(()) => {
                            total += entry.size();
                            if total > MAX_UNPACKED {
                                return Err(crate::tr!(
                                    "распакованное приложение больше {max} МБ",
                                    max = MAX_UNPACKED / 1024 / 1024
                                ));
                            }
                            continue;
                        }
                        Err(error) => {
                            tracing::debug!(target: "plugins", "«{}» is written, not linked: {error}", relative.display());
                        }
                    }
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| format!("«{}»: {error}", relative.display()))?;
                // What a header says of its size is not trusted: the bytes are counted as they come.
                let room = MAX_UNPACKED - total;
                let written = std::io::copy(&mut (&mut entry).take(room + 1), &mut file).map_err(broken)?;
                total += written;
                if total > MAX_UNPACKED {
                    return Err(crate::tr!(
                        "распакованное приложение больше {max} МБ",
                        max = MAX_UNPACKED / 1024 / 1024
                    ));
                }
            }
            other => {
                return Err(crate::tr!(
                    "«{path}»: в приложении только файлы и папки, а это {other}",
                    path = relative.display(),
                    other = format!("{other:?}")
                ));
            }
        }
    }
    if !into.join("manifest.json").is_file() {
        return Err(crate::tr!("в корне архива нет manifest.json"));
    }
    Ok(())
}

/// Removes a folder, gone already being fine.
fn remove_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The archive into PACKAGES_DIR as `<id>/<version>/`: checked, unpacked aside, read, renamed into place. The same
/// version already there is replaced only once the new one has read whole.
pub fn install_archive(root: &Path, bytes: &[u8], expected: Expected<'_>) -> Result<Installed, String> {
    install_archive_beside(root, bytes, expected, None)
}

/// The same beside the version installed (`beside`, counted before the room on the disk was asked): the files it has
/// already are hard links to its own.
pub fn install_archive_beside(
    root: &Path,
    bytes: &[u8],
    expected: Expected<'_>,
    previous: Option<&Beside>,
) -> Result<Installed, String> {
    if !valid_id(expected.id) || semver::Version::parse(expected.version).is_err() {
        return Err(crate::tr!("не то приложение: {id} {version}", id = expected.id, version = expected.version));
    }
    check_archive(bytes, expected.sha256, expected.size)?;
    std::fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let partial = root.join(format!(".partial-{}-{}", expected.id, expected.version));
    remove_dir(&partial).map_err(|error| format!("{}: {error}", partial.display()))?;
    let placed = (|| {
        unpack(bytes, &partial, previous)?;
        let package = match registry::read_package(&partial, expected.id, expected.version) {
            Candidate::Ready(package) => package,
            Candidate::Broken { problems, .. } => {
                return Err(crate::tr!("приложение не читается: {problems}", problems = problems.join("; ")))
            }
        };
        let target = root.join(expected.id).join(expected.version);
        std::fs::create_dir_all(root.join(expected.id)).map_err(|error| error.to_string())?;
        let old = root.join(format!(".old-{}-{}", expected.id, expected.version));
        let replacing = target.exists();
        if replacing {
            remove_dir(&old).map_err(|error| error.to_string())?;
            std::fs::rename(&target, &old).map_err(|error| error.to_string())?;
        }
        if let Err(error) = std::fs::rename(&partial, &target) {
            if replacing {
                let _ = std::fs::rename(&old, &target);
            }
            return Err(format!("{}: {error}", target.display()));
        }
        if replacing {
            let _ = remove_dir(&old);
        }
        Ok(Installed {
            id: expected.id.to_string(),
            version: expected.version.to_string(),
            kind: package.manifest.kind,
            dir: target,
        })
    })();
    if placed.is_err() {
        let _ = remove_dir(&partial);
        // A package folder made for this version alone and left empty goes too.
        let folder = root.join(expected.id);
        if std::fs::read_dir(&folder).is_ok_and(|mut entries| entries.next().is_none()) {
            let _ = std::fs::remove_dir(&folder);
        }
    }
    placed
}

/// Every version of a package but `keep`: an update's old ones, once the new one is in place. Only versions: a
/// folder whose name is not one — the app's own data `<id>/.data` (a VPN's connections), any dot folder — stays.
pub fn remove_other_versions(root: &Path, id: &str, keep: &str) -> std::io::Result<()> {
    if !valid_id(id) {
        return Ok(());
    }
    for entry in std::fs::read_dir(root.join(id))?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == keep || name.starts_with('.') || semver::Version::parse(name).is_err() {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// The newest `keep` versions of a package stay, older ones go: an update keeps the one before to fall back on.
pub fn keep_versions(root: &Path, id: &str, keep: usize) -> std::io::Result<()> {
    if !valid_id(id) {
        return Ok(());
    }
    let mut versions: Vec<(semver::Version, PathBuf)> = std::fs::read_dir(root.join(id))?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Some((semver::Version::parse(entry.file_name().to_str()?).ok()?, entry.path())))
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, dir) in versions.into_iter().skip(keep) {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// A package's folder, every version of it.
pub fn remove_package(root: &Path, id: &str) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, crate::tr!("не приложение: {id}", id = id)));
    }
    remove_dir(&root.join(id))
}

/// A download that did not come: why, in words, and whether the address said there is no such file.
#[derive(Debug, Clone)]
pub struct Failed {
    pub message: String,
    /// HTTP 404 or 410: no file at this address — a feed that moved on to a newer version, say.
    pub missing: bool,
    /// Larger than it may be: asking again brings the same.
    pub too_big: bool,
}

/// A file at `url`, whole, at most `max` bytes.
pub async fn download(url: &str, max: u64, timeout: Duration) -> Result<Vec<u8>, String> {
    fetch(url, max, timeout).await.map_err(|failed| failed.message)
}

/// The same, telling a file that is not there (404, 410) from a failure on the way.
pub async fn fetch(url: &str, max: u64, timeout: Duration) -> Result<Vec<u8>, Failed> {
    let failed = |message: String| Failed { message, missing: false, too_big: false };
    let client = crate::utils::http::client_builder()
        .timeout(timeout)
        .build()
        .map_err(|error| failed(crate::tr!("загрузка: {error}", error = error)))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| failed(crate::tr!("загрузка: {error}", error = crate::utils::http::why_failed(&error))))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failed {
            message: crate::tr!("загрузка: HTTP {status}", status = status),
            missing: matches!(status.as_u16(), 404 | 410),
            too_big: false,
        });
    }
    let too_big = || Failed {
        too_big: true, ..failed(crate::tr!("загрузка: файл больше {max} КБ", max = max / 1024))
    };
    if response.content_length().is_some_and(|length| length > max) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| failed(crate::tr!("загрузка: {error}", error = crate::utils::http::why_failed(&error))))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > max {
            return Err(too_big());
        }
    }
    Ok(bytes)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A file of a test archive: its path as the archive writes it, and what it holds.
    pub enum Item<'a> {
        File(&'a str, &'a [u8]),
        Dir(&'a str),
        Link(&'a str, &'a str),
    }

    /// A tar.gz written header by header: paths a well-behaved builder would refuse to write are written all the same.
    pub fn archive(items: &[Item<'_>]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for item in items {
            let mut header = tar::Header::new_gnu();
            let (path, data, kind): (&str, &[u8], tar::EntryType) = match item {
                Item::File(path, data) => (path, data, tar::EntryType::Regular),
                Item::Dir(path) => (path, b"", tar::EntryType::Directory),
                Item::Link(path, _) => (path, b"", tar::EntryType::Symlink),
            };
            // The raw name field: `set_path` refuses `..`, a hostile archive does not.
            let name = &mut header.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(kind);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            if let Item::Link(_, target) = item {
                header.set_link_name(target).unwrap();
            }
            header.set_cksum();
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    pub fn character_manifest(id: &str, version: &str) -> Value {
        json!({
            "schema": 1, "kind": "character", "id": id, "version": version, "title": "Артур", "api": "^2",
            "runs": ["pc", "station", "stationServer"], "license": "CC BY-NC 4.0, некоммерчески",
            "contributes": { "characters": [{ "id": id, "prompt": "Ты — Артур, дворецкий.", "gender": "male" }] }
        })
    }

    /// A character's `.vgp` with its manifest and an avatar.
    pub fn vgp(manifest: &Value) -> Vec<u8> {
        let text = manifest.to_string();
        archive(&[
            Item::File("manifest.json", text.as_bytes()),
            Item::Dir("assets/"),
            Item::File("assets/avatar.webp", b"RIFF\0\0\0\0WEBPVP8 "),
        ])
    }

    /// Bytes that do not pack, as an app's program nearly does not: `len` of them, the same for the same `seed`.
    pub fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    pub fn expected<'a>(id: &'a str, version: &'a str, bytes: &[u8], sha: &'a mut String) -> Expected<'a> {
        *sha = hex::encode(Sha256::digest(bytes));
        Expected { id, version, sha256: sha, size: bytes.len() as u64 }
    }

    /// Nothing but the packages themselves: no `.partial-`, no `.old-`.
    fn leftovers(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn puts_a_package_in_place_and_replaces_the_same_version_whole() {
        let root = tempfile::tempdir().unwrap();
        let bytes = vgp(&character_manifest("arthur", "1.2.0"));
        let mut sha = String::new();
        let installed = install_archive(root.path(), &bytes, expected("arthur", "1.2.0", &bytes, &mut sha)).unwrap();
        assert_eq!((installed.kind, installed.dir.clone()), (PackageKind::Character, root.path().join("arthur/1.2.0")));
        assert!(root.path().join("arthur/1.2.0/assets/avatar.webp").is_file());
        assert!(matches!(&registry::scan(root.path())["arthur"][0], Candidate::Ready(_)));
        // Again: the same version is replaced, nothing beside it.
        install_archive(root.path(), &bytes, expected("arthur", "1.2.0", &bytes, &mut sha)).unwrap();
        assert!(leftovers(root.path()).is_empty(), "{:?}", leftovers(root.path()));
        // An update beside it; the old one goes once the new one is there.
        let newer = vgp(&character_manifest("arthur", "1.3.0"));
        install_archive(root.path(), &newer, expected("arthur", "1.3.0", &newer, &mut sha)).unwrap();
        remove_other_versions(root.path(), "arthur", "1.3.0").unwrap();
        let versions: Vec<String> = registry::scan(root.path())["arthur"].iter().map(Candidate::version).collect();
        assert_eq!(versions, ["1.3.0"]);
        remove_package(root.path(), "arthur").unwrap();
        assert!(registry::scan(root.path()).is_empty());
        assert!(remove_package(root.path(), "../etc").is_err());
    }

    /// An update beside the version installed: the files the same as its own (path, size, bytes) are hard links to
    /// them and are not counted; one of the same size with other bytes, and the manifest, are written and counted.
    #[test]
    fn an_update_links_the_files_it_has_already_and_counts_only_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let program = noise(300_000, 1);
        let mut other = program.clone();
        other[299_999] ^= 1;
        let package = |version: &str, tail: &[u8]| {
            let text = character_manifest("arthur", version).to_string();
            archive(&[
                Item::File("manifest.json", text.as_bytes()),
                Item::Dir("assets/"),
                Item::File("assets/program", &program),
                Item::File("assets/tail", tail),
            ])
        };
        let mut sha = String::new();
        let first = package("1.2.0", &program[..]);
        install_archive(root.path(), &first, expected("arthur", "1.2.0", &first, &mut sha)).unwrap();
        let old = root.path().join("arthur/1.2.0");

        let fresh = beside(&first, None).unwrap();
        assert!(fresh.same.is_empty());
        assert!(fresh.changed_bytes > 2 * 300_000, "a first install writes every file");

        let update = package("1.3.0", &other[..]);
        let counted = beside(&update, Some(&old)).unwrap();
        assert_eq!(counted.same, BTreeSet::from([PathBuf::from("assets/program")]));
        let manifest = character_manifest("arthur", "1.3.0").to_string().len() as u64;
        assert_eq!(counted.changed_bytes, manifest + 300_000, "the manifest and the changed tail");
        assert_eq!(counted.changed_mb(), 1);

        install_archive_beside(root.path(), &update, expected("arthur", "1.3.0", &update, &mut sha), Some(&counted))
            .unwrap();
        let new = root.path().join("arthur/1.3.0");
        assert_eq!(std::fs::read(new.join("assets/program")).unwrap(), program);
        assert_eq!(std::fs::read(new.join("assets/tail")).unwrap(), other);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let inode = |path: PathBuf| std::fs::metadata(path).unwrap().ino();
            assert_eq!(inode(new.join("assets/program")), inode(old.join("assets/program")), "linked");
            assert_ne!(inode(new.join("assets/tail")), inode(old.join("assets/tail")), "written");
        }
        // The old version gone, the link holds its bytes.
        remove_other_versions(root.path(), "arthur", "1.3.0").unwrap();
        assert_eq!(std::fs::read(new.join("assets/program")).unwrap(), program);
        assert!(leftovers(root.path()).is_empty(), "{:?}", leftovers(root.path()));
    }

    /// The other versions go; the app's own data beside them (`.data`: a VPN's connections) and whatever is not a
    /// version stay.
    #[test]
    fn removing_the_other_versions_keeps_the_apps_data() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("own-server");
        for dir in ["0.1.3", "0.1.4", ".data/connections", "notes"] {
            std::fs::create_dir_all(app.join(dir)).unwrap();
        }
        std::fs::write(app.join(".data/connections/home.json"), b"{}").unwrap();
        remove_other_versions(root.path(), "own-server", "0.1.4").unwrap();
        assert!(!app.join("0.1.3").exists());
        assert!(app.join("0.1.4").is_dir() && app.join("notes").is_dir());
        assert_eq!(std::fs::read(app.join(".data/connections/home.json")).unwrap(), b"{}");
    }

    #[test]
    fn refuses_an_archive_that_is_not_the_catalogs_and_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let bytes = vgp(&character_manifest("arthur", "1.2.0"));
        let mut sha = String::new();
        let mut wrong_hash = expected("arthur", "1.2.0", &bytes, &mut sha);
        let other = "0".repeat(64);
        wrong_hash.sha256 = &other;
        let refused = install_archive(root.path(), &bytes, wrong_hash).unwrap_err();
        assert!(refused.contains("хеш"), "{refused}");
        let mut wrong_size = expected("arthur", "1.2.0", &bytes, &mut sha);
        wrong_size.size += 1;
        assert!(install_archive(root.path(), &bytes, wrong_size).unwrap_err().contains("размер"));
        // Another package under the catalog's name.
        let impostor = vgp(&character_manifest("palych", "1.2.0"));
        let mut sha2 = String::new();
        let mut named = expected("arthur", "1.2.0", &impostor, &mut sha2);
        named.id = "arthur";
        let refused = install_archive(root.path(), &impostor, named).unwrap_err();
        assert!(refused.contains("не совпадает с папкой"), "{refused}");
        assert!(registry::scan(root.path()).is_empty());
        assert!(leftovers(root.path()).is_empty() && !root.path().join("arthur").exists());
    }

    #[test]
    fn refuses_paths_out_links_svg_and_what_is_too_big_and_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let manifest = character_manifest("arthur", "1.0.0").to_string();
        let many: Vec<String> = (0..=MAX_FILES).map(|n| format!("assets/{n}.txt")).collect();
        let mut crowd = vec![Item::File("manifest.json", manifest.as_bytes())];
        crowd.extend(many.iter().map(|path| Item::File(path, b"x")));
        let big = vec![0u8; (MAX_UNPACKED + 1) as usize];
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (archive(&[Item::File("manifest.json", manifest.as_bytes()), Item::File("../evil", b"x")]), "ведёт из"),
            (archive(&[Item::File("manifest.json", manifest.as_bytes()), Item::File("/etc/evil", b"x")]), "ведёт из"),
            (
                archive(&[Item::File("manifest.json", manifest.as_bytes()), Item::Link("assets/x", "/etc/passwd")]),
                "только файлы и папки",
            ),
            (
                archive(&[Item::File("manifest.json", manifest.as_bytes()), Item::File("assets/a.svg", b"<svg/>")]),
                "SVG",
            ),
            (archive(&[Item::File("assets/avatar.webp", b"x")]), "нет manifest.json"),
            (archive(&crowd), "больше 500 файлов"),
            (
                archive(&[Item::File("manifest.json", manifest.as_bytes()), Item::File("assets/big", &big)]),
                "больше 32 МБ",
            ),
        ];
        for (bytes, why) in cases {
            let mut sha = String::new();
            // Unless the archive itself is too big, its hash and size are the catalog's: the contents are refused.
            let refused = install_archive(root.path(), &bytes, expected("arthur", "1.0.0", &bytes, &mut sha));
            let message = refused.unwrap_err();
            assert!(message.contains(why), "{why}: {message}");
            assert!(leftovers(root.path()).is_empty(), "{why}: {:?}", leftovers(root.path()));
            assert!(!root.path().join("arthur").exists(), "{why}");
        }
        assert!(!root.path().join("evil").exists() && !root.path().parent().unwrap().join("evil").exists());
        let huge = vec![0u8; (MAX_ARCHIVE + 1) as usize];
        assert!(check_archive(&huge, "", huge.len() as u64).unwrap_err().contains("больше 16 МБ"));
    }
}
