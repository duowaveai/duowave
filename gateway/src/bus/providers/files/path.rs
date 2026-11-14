//! Where an address's bytes are on the disk, checked (docs/plans/shared-folders.md, «Проверка пути и ссылок»). The
//! address's parts are plain names already (`bus/address.rs`: no `.`, `..`, `/`, `\`); here the disk has its word.
//! Each folder is its own fence: a link inside «Фото» that leads out of it — to the rest of the disk, or even to
//! «Документы» — is no file at all, so an app with a link to one folder never reads another through it. A part with
//! a dot, and the systems' own clutter, is hidden: neither listed nor read nor written. A name a Mac stored in NFD is
//! found as it was said or typed (NFC), and the other way round.
//!
//! A file is checked once more after it is opened: on a Mac and on Linux the opened file's own path must still lie in
//! the folder, so a link swapped in between the check and the opening is caught.

use std::fs::File;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Why a path is not given out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The folder itself is not there: a drive pulled out, an attached folder moved or renamed.
    Missing,
    /// No such file — or one the caller may not see: hidden, or a link out of the folder. They read the same.
    NotFound,
}

/// The systems' own files a person never means: Windows' thumbnails and folder settings, a Mac's folder icon.
const CLUTTER: &[&str] = &["Thumbs.db", "desktop.ini", "Icon\r", "$RECYCLE.BIN", "System Volume Information"];

/// A name a person does not see: dot files (`.DS_Store`, `.bin`) and the systems' clutter.
pub fn hidden(name: &str) -> bool {
    name.starts_with('.') || CLUTTER.iter().any(|known| name.eq_ignore_ascii_case(known))
}

/// The folder's own place as the disk knows it, every link resolved: what all below is checked against.
pub fn root(root: &Path) -> Result<PathBuf, Refused> {
    let real = root.canonicalize().map_err(|_| Refused::Missing)?;
    if real.is_dir() {
        Ok(real)
    } else {
        Err(Refused::Missing)
    }
}

/// `name` as the folder `at` holds it: as given, else in the other Unicode form.
fn named(at: &Path, name: &str) -> Option<PathBuf> {
    let given = at.join(name);
    if given.symlink_metadata().is_ok() {
        return Some(given);
    }
    let composed: String = name.nfc().collect();
    let decomposed: String = name.nfd().collect();
    [composed, decomposed]
        .into_iter()
        .filter(|other| other != name)
        .map(|other| at.join(other))
        .find(|other| other.symlink_metadata().is_ok())
}

/// An existing file or folder at `rest` below `root`: inside the folder after every link, no hidden part. The
/// folder itself for an empty `rest`.
pub fn existing(root: &Path, rest: &[String]) -> Result<PathBuf, Refused> {
    let base = self::root(root)?;
    if rest.iter().any(|part| hidden(part)) {
        return Err(Refused::NotFound);
    }
    let mut at = base.clone();
    for part in rest {
        at = named(&at, part).ok_or(Refused::NotFound)?;
    }
    let real = at.canonicalize().map_err(|_| Refused::NotFound)?;
    if real.starts_with(&base) {
        Ok(real)
    } else {
        Err(Refused::NotFound)
    }
}

/// Where a new or changed file at `rest` below `root` goes: the deepest folder that exists, checked like `existing`,
/// then plain new names. The last part is the file's own place — a link there is replaced, never followed (a file
/// is written beside and renamed over it).
pub fn writable(root: &Path, rest: &[String]) -> Result<PathBuf, Refused> {
    let base = self::root(root)?;
    if rest.is_empty() || rest.iter().any(|part| hidden(part)) {
        return Err(Refused::NotFound);
    }
    let (last, folders) = rest.split_last().ok_or(Refused::NotFound)?;
    let mut at = base.clone();
    let mut new = false;
    for part in folders {
        match named(&at, part).filter(|_| !new) {
            Some(found) => {
                let real = found.canonicalize().map_err(|_| Refused::NotFound)?;
                if !real.starts_with(&base) || !real.is_dir() {
                    return Err(Refused::NotFound);
                }
                at = real;
            }
            None => {
                new = true;
                at = at.join(part);
            }
        }
    }
    let file = if new { at.join(last) } else { named(&at, last).unwrap_or_else(|| at.join(last)) };
    Ok(file)
}

/// An entry itself at `rest` below `root`, to take away or move: its folders checked like `existing`, the entry not
/// followed (a link goes as a link) — and only one the folder's list shows: what is not seen is not there.
pub fn entry(root: &Path, rest: &[String]) -> Result<PathBuf, Refused> {
    let place = writable(root, rest)?;
    let base = self::root(root)?;
    if place.symlink_metadata().is_err() || !shown(&base, &place) {
        return Err(Refused::NotFound);
    }
    Ok(place)
}

/// Whether an entry seen in a folder's list may be shown: not hidden, and inside the folder after its links.
pub fn shown(base: &Path, entry: &Path) -> bool {
    let name = entry.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    if hidden(&name) {
        return false;
    }
    match entry.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => entry.canonicalize().is_ok_and(|real| real.starts_with(base)),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Opens an existing file at `rest` below `root` for reading, and checks the opened file is still the folder's.
pub fn open(root: &Path, rest: &[String]) -> Result<(File, PathBuf), Refused> {
    let path = existing(root, rest)?;
    let file = File::open(&path).map_err(|_| Refused::NotFound)?;
    let base = self::root(root)?;
    if !opened_inside(&file, &base) {
        return Err(Refused::NotFound);
    }
    Ok((file, path))
}

/// The opened file's own path lies in `base` (a Mac: F_GETPATH; Linux: /proc/self/fd). Elsewhere the check before
/// the opening stands alone.
#[cfg(target_os = "macos")]
pub fn opened_inside(file: &File, base: &Path) -> bool {
    use std::ffi::CStr;
    use std::os::unix::io::AsRawFd;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes and a NUL into the buffer, which is that long.
    let done = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) };
    if done == -1 {
        return false;
    }
    let Ok(path) = CStr::from_bytes_until_nul(&buffer) else { return false };
    let path = PathBuf::from(path.to_string_lossy().to_string());
    path.canonicalize().map(|real| real.starts_with(base)).unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn opened_inside(file: &File, base: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map(|path| path.starts_with(base)).unwrap_or(true)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn opened_inside(_file: &File, _base: &Path) -> bool {
    true
}

/// A place nothing is at yet: the path itself, else «Блины 2.txt», «Блины 3.txt»…
pub fn free(path: &Path) -> PathBuf {
    if path.symlink_metadata().is_err() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    let extension = path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
    (2..10_000)
        .map(|n| path.with_file_name(format!("{stem} {n}{extension}")))
        .find(|candidate| candidate.symlink_metadata().is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Moves a file or a folder; across disks (a drive and the home folder) by a copy, then the original removed.
pub fn move_any(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if crosses(&error) => {
            copy_any(from, to)?;
            remove_any(from)
        }
        Err(error) => Err(error),
    }
}

/// The error a rename gives across two disks.
fn crosses(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::CrossesDevices || error.raw_os_error() == Some(18)
}

/// Copies a file, or a folder with all in it — links copied as they are, never followed out.
pub fn copy_any(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        // A link is not followed: its target may lie outside every folder.
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_any(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Removes a file, a link or a folder with all in it.
pub fn remove_any(path: &Path) -> std::io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(path: &str) -> Vec<String> {
        path.split('/').filter(|part| !part.is_empty()).map(str::to_string).collect()
    }

    #[test]
    fn a_folder_is_its_own_fence() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("Фото");
        let docs = dir.path().join("Документы");
        std::fs::create_dir_all(photos.join("Отпуск")).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(photos.join("Отпуск/1.jpg"), "x").unwrap();
        std::fs::write(docs.join("паспорт.txt"), "secret").unwrap();
        std::fs::write(photos.join(".hidden"), "x").unwrap();
        assert!(existing(&photos, &parts("Отпуск/1.jpg")).is_ok());
        assert_eq!(existing(&photos, &parts(".hidden")), Err(Refused::NotFound));
        assert_eq!(existing(&photos, &parts("нет.jpg")), Err(Refused::NotFound));
        assert_eq!(existing(&dir.path().join("Флешка"), &[]), Err(Refused::Missing));
        #[cfg(unix)]
        {
            // A link out of the folder — to the rest of the disk, or to the next folder — is no file at all.
            std::os::unix::fs::symlink(&docs, photos.join("туда")).unwrap();
            std::os::unix::fs::symlink(docs.join("паспорт.txt"), photos.join("паспорт.txt")).unwrap();
            std::os::unix::fs::symlink("/etc", photos.join("etc")).unwrap();
            assert_eq!(existing(&photos, &parts("туда/паспорт.txt")), Err(Refused::NotFound));
            assert_eq!(existing(&photos, &parts("паспорт.txt")), Err(Refused::NotFound));
            assert_eq!(existing(&photos, &parts("etc/hosts")), Err(Refused::NotFound));
            assert_eq!(writable(&photos, &parts("туда/новый.txt")), Err(Refused::NotFound));
            assert!(!shown(&root(&photos).unwrap(), &photos.join("туда")));
            // A link inside the folder is fine.
            std::os::unix::fs::symlink(photos.join("Отпуск"), photos.join("Лето")).unwrap();
            assert!(existing(&photos, &parts("Лето/1.jpg")).is_ok());
            assert!(shown(&root(&photos).unwrap(), &photos.join("Лето")));
            let (_, opened) = open(&photos, &parts("Лето/1.jpg")).unwrap();
            assert!(opened.ends_with("Отпуск/1.jpg"));
        }
    }

    #[test]
    fn a_name_is_found_in_either_unicode_form() {
        let dir = tempfile::tempdir().unwrap();
        let decomposed: String = "Йошкар".nfd().collect();
        std::fs::create_dir_all(dir.path().join(&decomposed)).unwrap();
        std::fs::write(dir.path().join(&decomposed).join("a.txt"), "x").unwrap();
        let composed: String = "Йошкар".nfc().collect();
        assert!(existing(dir.path(), &[composed.clone(), "a.txt".into()]).is_ok());
        let place = writable(dir.path(), &[composed, "b.txt".into()]).unwrap();
        assert!(place.parent().unwrap().exists(), "into the folder that is there, not a twin of it");
    }

    #[test]
    fn new_folders_are_made_below_the_last_one_there() {
        let dir = tempfile::tempdir().unwrap();
        let place = writable(dir.path(), &parts("a/b/c.txt")).unwrap();
        assert!(place.ends_with("a/b/c.txt"));
        assert_eq!(writable(dir.path(), &[]), Err(Refused::NotFound));
        assert_eq!(writable(dir.path(), &parts("a/.git/x")), Err(Refused::NotFound));
    }

    #[test]
    fn a_move_and_a_copy_never_follow_a_link_out() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        std::fs::create_dir_all(from.join("inner")).unwrap();
        std::fs::write(from.join("inner/a.txt"), "a").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", from.join("etc")).unwrap();
        copy_any(&from, &dir.path().join("copy")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("copy/inner/a.txt")).unwrap(), "a");
        assert!(!dir.path().join("copy/etc").exists());
        move_any(&dir.path().join("copy"), &dir.path().join("moved")).unwrap();
        assert!(dir.path().join("moved/inner/a.txt").exists() && !dir.path().join("copy").exists());
    }
}
