//! The history's store (docs/plans/package-history.md, «Хранилище»): objects by their SHA-256, as git keeps them, and
//! the small mutable parts beside them — the marks (`refs`) and each app's line of records (`logs`).
//!
//! - An object is `«kind» «length»\0` and its content, deflated; its name is the SHA-256 of the whole before deflating.
//!   Kinds: `file` (a file as it is), `tree` (an app's files by path), `step` (a record), `data` (a plugin's data).
//!   Reading checks the hash: a spoilt object counts as gone. One written twice is the same object: nothing to merge.
//! - Every write goes into a temporary file first and is renamed into place: a half-written object or mark never is.
//! - `refs/<app>/<name>` — a mark: the id of a record (`draft`, `installed`, `catalog`); `remotes/<peer>/<app>/<name>`
//!   — the marks of another gateway (the station's, on the computer), never mixed with ours.
//! - `logs/<app>` — the app's records in the order they were made, one id a line: the feed the panel shows. The graph
//!   (`parents`) is for merging; the feed is this line.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The store's format; written once into `format`.
pub const FORMAT: &str = "1";
/// An object holds at most this much: a picture on a computer is up to 10 MB (plugins/files.rs).
pub const MAX_OBJECT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Tree,
    Step,
    Data,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Tree => "tree",
            Kind::Step => "step",
            Kind::Data => "data",
        }
    }

    pub fn parse(tag: &str) -> Option<Kind> {
        match tag {
            "file" => Some(Kind::File),
            "tree" => Some(Kind::Tree),
            "step" => Some(Kind::Step),
            "data" => Some(Kind::Data),
            _ => None,
        }
    }
}

/// An object's name: 64 lower-case hex digits.
pub fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A mark's, an app's or a peer's name: plain latin letters, digits, «-», «_», «.», not starting with a dot.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn header(kind: Kind, length: usize) -> Vec<u8> {
    format!("{} {length}\0", kind.tag()).into_bytes()
}

/// The name an object of this kind and content gets.
pub fn id_of(kind: Kind, content: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(header(kind, content.len()));
    digest.update(content);
    hex::encode(digest.finalize())
}

fn io_error(text: impl Into<String>) -> std::io::Error {
    std::io::Error::other(text.into())
}

/// The whole of `bytes` into `path`, through a temporary file beside it.
pub fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("object");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", hex::encode(crate::utils::ids::random_bytes(4))));
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// A deflated object opened: its kind and content, when its name is its hash.
fn inflate(id: &str, packed: &[u8]) -> Result<(Kind, Vec<u8>), String> {
    let mut whole = Vec::new();
    flate2::read::DeflateDecoder::new(packed)
        .take(MAX_OBJECT as u64 + 64)
        .read_to_end(&mut whole)
        .map_err(|error| format!("не распаковался: {error}"))?;
    let zero = whole.iter().position(|byte| *byte == 0).ok_or("нет заголовка")?;
    let head = std::str::from_utf8(&whole[..zero]).map_err(|_| "заголовок не текст")?;
    let (tag, length) = head.split_once(' ').ok_or("заголовок не тот")?;
    let kind = Kind::parse(tag).ok_or("вид не тот")?;
    let length: usize = length.parse().map_err(|_| "длина не число")?;
    if length != whole.len() - zero - 1 || length > MAX_OBJECT {
        return Err("длина не сходится".into());
    }
    if hex::encode(Sha256::digest(&whole)) != id {
        return Err("хеш не сходится".into());
    }
    Ok((kind, whole[zero + 1..].to_vec()))
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store in `root`: a folder of another format is refused; a missing one is made at the first write, so a
    /// gateway (or a test) that never writes leaves nothing behind.
    pub fn open(root: PathBuf) -> std::io::Result<Store> {
        match std::fs::read_to_string(root.join("format")) {
            Ok(text) if text.trim() == FORMAT => {}
            Ok(text) => return Err(io_error(format!("история формата {}, а шлюз знает {FORMAT}", text.trim()))),
            Err(_) if root.join("objects").is_dir() => {
                return Err(io_error("в папке истории нет файла format"));
            }
            Err(_) => {}
        }
        Ok(Store { root })
    }

    /// The folder and its format, before the first write.
    fn made(&self) -> std::io::Result<()> {
        let format = self.root.join("format");
        if !format.is_file() {
            write_whole(&format, FORMAT.as_bytes())?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.root.join("objects").join(&id[..2]).join(&id[2..])
    }

    /// The object put in, deflated; its name. Already there — nothing is written.
    pub fn put(&self, kind: Kind, content: &[u8]) -> std::io::Result<String> {
        if content.len() > MAX_OBJECT {
            return Err(io_error(format!("объект больше {} МБ", MAX_OBJECT / 1024 / 1024)));
        }
        let id = id_of(kind, content);
        let path = self.path_of(&id);
        if path.is_file() {
            return Ok(id);
        }
        self.made()?;
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&header(kind, content.len()))?;
        encoder.write_all(content)?;
        write_whole(&path, &encoder.finish()?)?;
        Ok(id)
    }

    /// The object: its kind and content. None — not there, or spoilt (it says so in the log).
    pub fn get(&self, id: &str) -> Option<(Kind, Vec<u8>)> {
        if !valid_id(id) {
            return None;
        }
        let packed = std::fs::read(self.path_of(id)).ok()?;
        match inflate(id, &packed) {
            Ok(found) => Some(found),
            Err(why) => {
                tracing::warn!(target: "plugins", "history: object {id} is spoilt ({why}); it counts as gone");
                None
            }
        }
    }

    /// The object, when it is of this kind.
    pub fn get_kind(&self, id: &str, kind: Kind) -> Option<Vec<u8>> {
        self.get(id).filter(|(found, _)| *found == kind).map(|(_, content)| content)
    }

    pub fn has(&self, id: &str) -> bool {
        valid_id(id) && self.path_of(id).is_file()
    }

    /// The object as it lies on disk, deflated: what goes to another gateway.
    pub fn packed(&self, id: &str) -> Option<Vec<u8>> {
        if !valid_id(id) {
            return None;
        }
        std::fs::read(self.path_of(id)).ok()
    }

    /// An object from another gateway, deflated as it was sent: put in only when its name is its hash. Its kind.
    pub fn put_packed(&self, id: &str, packed: &[u8]) -> Result<Kind, String> {
        if !valid_id(id) {
            return Err("имя объекта не то".into());
        }
        let (kind, _) = inflate(id, packed)?;
        let path = self.path_of(id);
        if !path.is_file() {
            self.made().map_err(|error| format!("не записался: {error}"))?;
            write_whole(&path, packed).map_err(|error| format!("не записался: {error}"))?;
        }
        Ok(kind)
    }

    pub fn remove(&self, id: &str) {
        if valid_id(id) {
            let _ = std::fs::remove_file(self.path_of(id));
        }
    }

    /// Every object's name and its size on disk.
    pub fn objects(&self) -> Vec<(String, u64)> {
        let mut found = Vec::new();
        for prefix in std::fs::read_dir(self.root.join("objects")).into_iter().flatten().flatten() {
            let Some(head) = prefix.file_name().to_str().map(str::to_string) else { continue };
            if head.len() != 2 {
                continue;
            }
            for entry in std::fs::read_dir(prefix.path()).into_iter().flatten().flatten() {
                let Some(tail) = entry.file_name().to_str().map(str::to_string) else { continue };
                let id = format!("{head}{tail}");
                if valid_id(&id) {
                    found.push((id, entry.metadata().map(|meta| meta.len()).unwrap_or(0)));
                }
            }
        }
        found
    }

    /// What the store takes on disk, objects and all.
    pub fn bytes(&self) -> u64 {
        fn walk(at: &Path) -> u64 {
            std::fs::read_dir(at)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| match entry.file_type() {
                    Ok(kind) if kind.is_dir() => walk(&entry.path()),
                    Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.root)
    }

    // The marks.

    fn ref_path(&self, peer: Option<&str>, app: &str, name: &str) -> Option<PathBuf> {
        if !valid_name(app) || !valid_name(name) || peer.is_some_and(|peer| !valid_name(peer)) {
            return None;
        }
        Some(match peer {
            Some(peer) => self.root.join("remotes").join(peer).join(app).join(name),
            None => self.root.join("refs").join(app).join(name),
        })
    }

    pub fn ref_get(&self, app: &str, name: &str) -> Option<String> {
        self.remote_ref_get(None, app, name)
    }

    pub fn remote_ref_get(&self, peer: Option<&str>, app: &str, name: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.ref_path(peer, app, name)?).ok()?;
        let id = text.trim().to_string();
        valid_id(&id).then_some(id)
    }

    /// A mark set to `id`, or taken away (None).
    pub fn ref_set(&self, app: &str, name: &str, id: Option<&str>) -> std::io::Result<()> {
        self.remote_ref_set(None, app, name, id)
    }

    pub fn remote_ref_set(&self, peer: Option<&str>, app: &str, name: &str, id: Option<&str>) -> std::io::Result<()> {
        let path = self.ref_path(peer, app, name).ok_or_else(|| io_error("имя метки не то"))?;
        if id.is_some() {
            self.made()?;
        }
        match id {
            Some(id) if valid_id(id) => write_whole(&path, format!("{id}\n").as_bytes()),
            Some(_) => Err(io_error("метка не на объект")),
            None => match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }

    /// An app's marks by name.
    pub fn refs(&self, app: &str) -> BTreeMap<String, String> {
        self.remote_refs(None, app)
    }

    pub fn remote_refs(&self, peer: Option<&str>, app: &str) -> BTreeMap<String, String> {
        let Some(folder) = self.ref_path(peer, app, "x").and_then(|path| path.parent().map(Path::to_path_buf)) else {
            return BTreeMap::new();
        };
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let id = self.remote_ref_get(peer, app, &name)?;
                Some((name, id))
            })
            .collect()
    }

    /// The peers whose marks are kept here.
    pub fn peers(&self) -> Vec<String> {
        names_in(&self.root.join("remotes"))
    }

    /// The apps a peer's marks name.
    pub fn peer_apps(&self, peer: &str) -> Vec<String> {
        if !valid_name(peer) {
            return Vec::new();
        }
        names_in(&self.root.join("remotes").join(peer))
    }

    /// A peer's marks of an app all go.
    pub fn forget_remote(&self, peer: &str, app: &str) {
        if valid_name(peer) && valid_name(app) {
            let _ = std::fs::remove_dir_all(self.root.join("remotes").join(peer).join(app));
        }
    }

    // The lines of records.

    fn log_path(&self, app: &str) -> Option<PathBuf> {
        valid_name(app).then(|| self.root.join("logs").join(app))
    }

    /// The app's records, the oldest first.
    pub fn log(&self, app: &str) -> Vec<String> {
        let Some(path) = self.log_path(app) else { return Vec::new() };
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|id| valid_id(id))
            .map(str::to_string)
            .collect()
    }

    /// The app's line written whole.
    pub fn log_write(&self, app: &str, ids: &[String]) -> std::io::Result<()> {
        let path = self.log_path(app).ok_or_else(|| io_error("имя приложения не то"))?;
        if ids.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        self.made()?;
        let mut text = ids.join("\n");
        text.push('\n');
        write_whole(&path, text.as_bytes())
    }

    /// A record at the end of the app's line; one already in it stays where it is.
    pub fn log_push(&self, app: &str, id: &str) -> std::io::Result<()> {
        let mut ids = self.log(app);
        if ids.iter().any(|known| known == id) {
            return Ok(());
        }
        ids.push(id.to_string());
        self.log_write(app, &ids)
    }

    /// The last record of the line replaced by another (two saves by hand within minutes are one step).
    pub fn log_replace_last(&self, app: &str, old: &str, new: &str) -> std::io::Result<()> {
        let mut ids = self.log(app);
        match ids.last_mut() {
            Some(last) if last == old => *last = new.to_string(),
            _ => ids.push(new.to_string()),
        }
        self.log_write(app, &ids)
    }

    /// The apps with a line or a mark here.
    pub fn apps(&self) -> Vec<String> {
        let mut apps = names_in(&self.root.join("logs"));
        for app in names_in(&self.root.join("refs")) {
            if !apps.contains(&app) {
                apps.push(app);
            }
        }
        apps.sort();
        apps
    }

    /// An app's line and marks all go (its objects go with the cleaning).
    pub fn forget_app(&self, app: &str) {
        if valid_name(app) {
            let _ = std::fs::remove_file(self.root.join("logs").join(app));
            let _ = std::fs::remove_dir_all(self.root.join("refs").join(app));
            let _ = std::fs::remove_file(self.root.join("removed").join(format!("{app}.json")));
        }
    }

    // Removed apps: their history lives on for a while.

    /// When the app was removed, RFC 3339; None — it was not.
    pub fn removed_at(&self, app: &str) -> Option<String> {
        if !valid_name(app) {
            return None;
        }
        let text = std::fs::read_to_string(self.root.join("removed").join(format!("{app}.json"))).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        value.get("at").and_then(|at| at.as_str()).map(str::to_string)
    }

    pub fn set_removed(&self, app: &str, at: Option<&str>) -> std::io::Result<()> {
        if !valid_name(app) {
            return Err(io_error("имя приложения не то"));
        }
        let path = self.root.join("removed").join(format!("{app}.json"));
        match at {
            Some(at) => write_whole(&path, serde_json::json!({ "at": at }).to_string().as_bytes()),
            None => {
                let _ = std::fs::remove_file(path);
                Ok(())
            }
        }
    }
}

fn names_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| valid_name(name))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_an_object_once_by_its_hash_and_finds_a_spoilt_one_gone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("history")).unwrap();
        let id = store.put(Kind::File, b"export function hello() {}\n").unwrap();
        assert!(valid_id(&id));
        assert_eq!(
            store.put(Kind::File, b"export function hello() {}\n").unwrap(),
            id,
            "the same content, the same id"
        );
        assert_ne!(id_of(Kind::Data, b"export function hello() {}\n"), id, "the kind is in the hash");
        assert_eq!(store.get(&id), Some((Kind::File, b"export function hello() {}\n".to_vec())));
        assert_eq!(store.get_kind(&id, Kind::Tree), None);
        assert_eq!(store.objects().len(), 1);

        // Another gateway's copy goes in only when its name is its hash.
        let other = tempfile::tempdir().unwrap();
        let peer = Store::open(other.path().to_path_buf()).unwrap();
        assert_eq!(peer.put_packed(&id, &store.packed(&id).unwrap()), Ok(Kind::File));
        assert!(peer.put_packed(&id_of(Kind::File, b"else"), &store.packed(&id).unwrap()).is_err());

        // A spoilt object counts as gone.
        let path = store.path_of(&id);
        let mut packed = std::fs::read(&path).unwrap();
        let last = packed.len() - 1;
        packed[last] ^= 0xFF;
        std::fs::write(&path, packed).unwrap();
        assert_eq!(store.get(&id), None);
        // Another format is not read as ours.
        std::fs::write(dir.path().join("history/format"), "2").unwrap();
        assert!(Store::open(dir.path().join("history")).is_err());
    }

    #[test]
    fn keeps_marks_lines_and_removed_apps_apart_for_each_app_and_peer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        let (a, b) = (store.put(Kind::Step, b"a").unwrap(), store.put(Kind::Step, b"b").unwrap());
        store.ref_set("weather", "draft", Some(&a)).unwrap();
        store.remote_ref_set(Some("kitchen"), "weather", "draft", Some(&b)).unwrap();
        assert_eq!(store.ref_get("weather", "draft"), Some(a.clone()));
        assert_eq!(store.remote_ref_get(Some("kitchen"), "weather", "draft"), Some(b.clone()));
        assert!(
            store.ref_set("../etc", "draft", Some(&a)).is_err() && store.ref_set("weather", "x", Some("y")).is_err()
        );
        store.ref_set("weather", "draft", None).unwrap();
        store.ref_set("weather", "draft", None).unwrap();
        assert!(store.refs("weather").is_empty());
        assert_eq!(store.peers(), vec!["kitchen".to_string()]);

        store.log_push("weather", &a).unwrap();
        store.log_push("weather", &a).unwrap();
        store.log_push("weather", &b).unwrap();
        assert_eq!(store.log("weather"), vec![a.clone(), b.clone()]);
        let c = store.put(Kind::Step, b"c").unwrap();
        store.log_replace_last("weather", &b, &c).unwrap();
        assert_eq!(store.log("weather"), vec![a.clone(), c.clone()]);
        assert_eq!(store.apps(), vec!["weather".to_string()]);

        store.set_removed("weather", Some("2026-11-12T10:00:00Z")).unwrap();
        assert_eq!(store.removed_at("weather").as_deref(), Some("2026-11-12T10:00:00Z"));
        store.forget_app("weather");
        assert!(store.log("weather").is_empty() && store.removed_at("weather").is_none());
    }
}
