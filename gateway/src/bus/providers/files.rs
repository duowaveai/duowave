//! Folders and files on the bus (`files/…`, docs/plans/bus.md, «Папки»). A folder is an address like any other, on
//! the same ladder: see — its list, listen — «a file came», call — read, edit — put, change, remove. The rights are the
//! bus's: this provider has none of its own, it only knows where the bytes live and keeps them in bounds.
//!
//! - `files/app/<id>/…` — an app's own folder: free for it, unseen by others; `tmp/` in it is cleaned by itself.
//! - `files/home/<папка>/…` — the household's: Картинки, Фото, Видео, Документы, Загрузки, and Музыка and Сказки,
//!   which are the household's own music and tales (CONTENT_DIR), read only.
//! - `files/person/<id>/…` — a person's: personal, open only to a link made for it.
//! - `files/project/<id>/…` — a project's.
//!
//! The place of each is a row of `Places` — on this gateway's disk now; a folder on another gateway is a new kind of
//! place, not a new address. Removing from a shared or a personal folder puts the file in the bin for a week.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};

use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, Participant, Provider};
use crate::bus::{Bus, Level};

/// The household's folders made at the start.
pub const HOME_FOLDERS: &[&str] = &["Картинки", "Фото", "Видео", "Документы", "Загрузки"];
/// The household's music and tales: CONTENT_DIR's folders, read only here.
const CONTENT_FOLDERS: &[(&str, &str)] = &[("Музыка", "music"), ("Сказки", "tales")];
/// How long a file waits in `tmp/` before it is cleaned away.
pub const TMP_TTL: Duration = Duration::from_secs(24 * 3600);
/// How long a removed file waits in the bin.
pub const BIN_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// A file read or written through a letter at most: JSON carries it, the app's memory holds it.
pub const COMPUTER_FILE_MAX: u64 = 8 * 1024 * 1024;
pub const STATION_FILE_MAX: u64 = 1024 * 1024;
/// On the station all the files together, apps' and the household's: its disk is small (decision 6).
pub const STATION_ALL_MAX: u64 = 5 * 1024 * 1024;
/// An app's own folder on a computer when its manifest names no other.
pub const APP_FOLDER_MB: u64 = 64;

/// Where the bytes of an address live, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    /// The folder participant: `files/home/Картинки`, `files/app/<id>`.
    pub folder: Address,
    /// Its directory on this disk.
    pub root: PathBuf,
    /// The path below it.
    pub rest: Vec<String>,
    pub read_only: bool,
    pub private: bool,
    pub owner: Option<Address>,
}

impl Spot {
    pub fn path(&self) -> PathBuf {
        let mut path = self.root.clone();
        for part in &self.rest {
            path.push(part);
        }
        path
    }
}

/// The places of the folders on this gateway.
#[derive(Debug, Clone)]
pub struct Places {
    /// `DATA_DIR/files`.
    pub files: PathBuf,
    /// `DATA_DIR/plugin-files`: the apps' own folders, their pictures at the top.
    pub apps: PathBuf,
    /// CONTENT_DIR.
    pub content: PathBuf,
    /// The station's limits.
    pub station: bool,
}

impl Places {
    pub fn new(data_dir: &Path, content: &Path, station: bool) -> Places {
        Places {
            files: data_dir.join("files"),
            apps: data_dir.join("plugin-files"),
            content: content.to_path_buf(),
            station,
        }
    }

    /// Where an address lives; none for an address no folder holds.
    pub fn spot(&self, address: &Address) -> Option<Spot> {
        if address.kind() != address::FILES {
            return None;
        }
        let parts: Vec<&str> = address.segments().collect();
        let (area, name) = (*parts.first()?, *parts.get(1)?);
        let folder = Address::parse(&format!("files/{area}/{name}")).ok()?;
        let rest = parts[2..].iter().map(|part| part.to_string()).collect();
        let (root, read_only, private, owner) = match area {
            // An app's own folder is where its pictures always were (plugins/files.rs): they are its top.
            "app" => (self.apps.join(name), false, false, Some(Address::app(name))),
            "home" => match CONTENT_FOLDERS.iter().find(|(title, _)| *title == name) {
                Some((_, folder)) => (self.content.join(folder), true, false, None),
                None => (self.files.join("home").join(name), false, false, None),
            },
            "person" => (self.files.join("person").join(name), false, true, Address::new(address::PERSON, name).ok()),
            "project" => (self.files.join("project").join(name), false, false, None),
            _ => return None,
        };
        Some(Spot { folder, root, rest, read_only, private, owner })
    }

    /// The household's folders, made where missing.
    pub fn prepare(&self) {
        for name in HOME_FOLDERS {
            let _ = std::fs::create_dir_all(self.files.join("home").join(name));
        }
    }

    /// The bytes of every file below a directory.
    fn size_of(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|entry| match entry.metadata() {
                Ok(meta) if meta.is_dir() => Places::size_of(&entry.path()),
                Ok(meta) => meta.len(),
                Err(_) => 0,
            })
            .sum()
    }

    /// Cleans every app's `tmp/` of what is older than its time, and the bin of what waited its week.
    pub fn sweep(&self) -> usize {
        let mut removed = 0;
        let now = SystemTime::now();
        let old = |path: &Path, ttl: Duration| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > ttl)
        };
        if let Ok(apps) = std::fs::read_dir(&self.apps) {
            for app in apps.flatten() {
                let tmp = app.path().join("tmp");
                for entry in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
                    if old(&entry.path(), TMP_TTL) && remove_any(&entry.path()).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        for entry in std::fs::read_dir(self.files.join(".bin")).into_iter().flatten().flatten() {
            if old(&entry.path(), BIN_TTL) && remove_any(&entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    name: String,
    address: String,
    folder: bool,
    size: u64,
    modified: Option<String>,
}

fn item(address: &Address, path: &Path) -> Option<Item> {
    let meta = std::fs::metadata(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    Some(Item {
        name: name.clone(),
        address: address.to_string(),
        folder: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        modified: meta.modified().ok().map(|time| crate::utils::time::iso(chrono::DateTime::<chrono::Utc>::from(time))),
    })
}

/// The folders' provider.
pub struct Files {
    places: Places,
    bus: OnceLock<Weak<Bus>>,
    /// An app's own folder in MB (its manifest's `budget.filesMb`).
    quota: Arc<dyn Fn(&str) -> u64 + Send + Sync>,
}

impl Files {
    pub fn new(places: Places, quota: Arc<dyn Fn(&str) -> u64 + Send + Sync>) -> Files {
        Files { places, bus: OnceLock::new(), quota }
    }

    /// The bus it tells its news on and checks a move's destination with.
    pub fn set_bus(&self, bus: Weak<Bus>) {
        let _ = self.bus.set(bus);
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    fn bus(&self) -> Option<Arc<Bus>> {
        self.bus.get().and_then(Weak::upgrade)
    }

    fn max_file(&self) -> u64 {
        if self.places.station {
            STATION_FILE_MAX
        } else {
            COMPUTER_FILE_MAX
        }
    }

    /// Whether `bytes` more fit where the spot is: the app's own folder within its quota, the station's files within
    /// its total.
    fn room(&self, spot: &Spot, bytes: u64) -> Result<(), BusError> {
        if self.places.station {
            let used = Places::size_of(&self.places.files);
            if used + bytes > STATION_ALL_MAX {
                return Err(BusError::Failed(format!(
                    "на колонке места под файлы нет: занято {} КБ из {} КБ",
                    used / 1024,
                    STATION_ALL_MAX / 1024
                )));
            }
        }
        if let Some(app) = spot.owner.as_ref().filter(|owner| owner.kind() == address::APP).and_then(Address::name) {
            let quota = (self.quota)(app) * 1024 * 1024;
            let used = Places::size_of(&spot.root);
            if used + bytes > quota {
                return Err(BusError::Failed(format!(
                    "своя папка полна: занято {} КБ из {} КБ",
                    used / 1024,
                    quota / 1024
                )));
            }
        }
        Ok(())
    }

    fn tell(&self, letter: &Letter, folder: &Address, event: &str, file: &Address) {
        if let Some(bus) = self.bus() {
            bus.tell(letter.trail.clone(), folder.clone(), event, json!({ "file": file.to_string() }));
        }
    }

    fn act(&self, letter: &Letter) -> Result<Value, BusError> {
        let spot = self.places.spot(&letter.to).ok_or_else(|| BusError::NotFound(letter.to.to_string()))?;
        let path = spot.path();
        let input = &letter.input;
        let writes = matches!(letter.handle.as_str(), "write" | "remove" | "move" | "mkdir");
        if writes && spot.read_only {
            return Err(BusError::Denied(format!("«{}» только для чтения", spot.folder.path())));
        }
        match letter.handle.as_str() {
            "list" => {
                let entries = std::fs::read_dir(&path).map_err(|_| BusError::NotFound(letter.to.to_string()))?;
                let mut items: Vec<Item> = entries
                    .flatten()
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        item(&letter.to.join(&name).ok()?, &entry.path())
                    })
                    .collect();
                items.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(json!({ "items": items }))
            }
            "stat" => {
                item(&letter.to, &path).map(|item| json!(item)).ok_or_else(|| BusError::NotFound(letter.to.to_string()))
            }
            "read" => {
                let meta = std::fs::metadata(&path).map_err(|_| BusError::NotFound(letter.to.to_string()))?;
                if meta.is_dir() {
                    return Err(BusError::Failed("это папка: её список — list".into()));
                }
                if meta.len() > self.max_file() {
                    return Err(BusError::Failed(format!(
                        "файл больше {} МБ: такой читают по частям или ссылкой",
                        self.max_file() / 1024 / 1024
                    )));
                }
                let bytes = std::fs::read(&path).map_err(|error| BusError::Failed(error.to_string()))?;
                let as_text = input.get("as").and_then(Value::as_str) == Some("text");
                Ok(match (as_text, String::from_utf8(bytes.clone())) {
                    (true, Ok(text)) => json!({ "text": text, "size": bytes.len() }),
                    (true, Err(_)) => return Err(BusError::Failed("файл не текст: читайте без as: text".into())),
                    (false, _) => json!({ "base64": B64.encode(&bytes), "size": bytes.len() }),
                })
            }
            "write" => {
                if spot.rest.is_empty() {
                    return Err(BusError::Failed("write: нужно имя файла в папке".into()));
                }
                let bytes =
                    match (input.get("text").and_then(Value::as_str), input.get("base64").and_then(Value::as_str)) {
                        (Some(text), _) => text.as_bytes().to_vec(),
                        (None, Some(data)) => B64
                            .decode(data.split_once(',').map(|(_, data)| data).unwrap_or(data))
                            .map_err(|_| BusError::Failed("base64 не читается".into()))?,
                        (None, None) => return Err(BusError::Failed("write: {text} или {base64}".into())),
                    };
                if bytes.len() as u64 > self.max_file() {
                    return Err(BusError::Failed(format!("файл больше {} МБ", self.max_file() / 1024 / 1024)));
                }
                let before = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                self.room(&spot, (bytes.len() as u64).saturating_sub(before))?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| BusError::Failed(error.to_string()))?;
                }
                // A temporary file renamed into place: a half-written file is never seen.
                let temp = path.with_extension(format!("part-{}", crate::utils::ids::random_hex(4)));
                std::fs::write(&temp, &bytes).map_err(|error| BusError::Failed(error.to_string()))?;
                std::fs::rename(&temp, &path).map_err(|error| BusError::Failed(error.to_string()))?;
                self.tell(letter, &spot.folder, if before == 0 { "file.added" } else { "file.changed" }, &letter.to);
                Ok(json!({ "address": letter.to.to_string(), "size": bytes.len() }))
            }
            "mkdir" => {
                std::fs::create_dir_all(&path).map_err(|error| BusError::Failed(error.to_string()))?;
                Ok(json!({ "address": letter.to.to_string() }))
            }
            "remove" => {
                if spot.rest.is_empty() {
                    return Err(BusError::Denied("саму папку не удалить".into()));
                }
                if !path.exists() {
                    return Err(BusError::NotFound(letter.to.to_string()));
                }
                let own = spot.owner.as_ref().is_some_and(|owner| owner.kind() == address::APP);
                if own {
                    remove_any(&path).map_err(|error| BusError::Failed(error.to_string()))?;
                } else {
                    // The household's and a person's files wait a week in the bin.
                    let bin = self.places.files.join(".bin").join(crate::utils::ids::random_hex(6));
                    std::fs::create_dir_all(&bin).map_err(|error| BusError::Failed(error.to_string()))?;
                    let name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
                    std::fs::rename(&path, bin.join(name)).map_err(|error| BusError::Failed(error.to_string()))?;
                }
                self.tell(letter, &spot.folder, "file.removed", &letter.to);
                Ok(json!({ "removed": letter.to.to_string(), "bin": !own }))
            }
            "move" => {
                let to = input
                    .get("to")
                    .and_then(Value::as_str)
                    .and_then(|to| Address::parse(to).ok())
                    .ok_or_else(|| BusError::Failed("move: {to: адрес, куда}".into()))?;
                let target = self.places.spot(&to).ok_or_else(|| BusError::NotFound(to.to_string()))?;
                if target.read_only || target.rest.is_empty() {
                    return Err(BusError::Denied(format!("в «{}» не положить", target.folder.path())));
                }
                let size = Places::size_of(&path).max(std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0));
                if target.root != spot.root {
                    self.room(&target, size)?;
                }
                let destination = target.path();
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| BusError::Failed(error.to_string()))?;
                }
                std::fs::rename(&path, &destination).map_err(|error| BusError::Failed(error.to_string()))?;
                self.tell(letter, &spot.folder, "file.removed", &letter.to);
                self.tell(letter, &target.folder, "file.added", &to);
                Ok(json!({ "address": to.to_string() }))
            }
            other => Err(BusError::NotFound(format!("{}: {other}", letter.to))),
        }
    }
}

impl Provider for Files {
    fn kind(&self) -> &'static str {
        address::FILES
    }

    fn participants(&self) -> Vec<Participant> {
        let mut folders = Vec::new();
        let mut add = |area: &str, name: &str| {
            if let Ok(address) = Address::parse(&format!("files/{area}/{name}")) {
                if let Some(participant) = self.participant(&address) {
                    folders.push(participant);
                }
            }
        };
        for name in HOME_FOLDERS {
            add("home", name);
        }
        for (name, _) in CONTENT_FOLDERS {
            add("home", name);
        }
        for area in ["person", "project"] {
            for entry in std::fs::read_dir(self.places.files.join(area)).into_iter().flatten().flatten() {
                add(area, &entry.file_name().to_string_lossy());
            }
        }
        folders
    }

    fn participant(&self, address: &Address) -> Option<Participant> {
        let spot = self.places.spot(address)?;
        // A name the questions put in quotes: «менять файлы в папке «Документы»».
        let title = match spot.folder.segments().collect::<Vec<_>>().as_slice() {
            ["app", id] => self.bus().map(|bus| bus.title_of(&Address::app(id))).unwrap_or_else(|| id.to_string()),
            ["home", name] => name.to_string(),
            ["person", id] => format!("личное {id}"),
            ["project", id] => format!("проект {id}"),
            _ => spot.folder.to_string(),
        };
        Some(Participant { address: spot.folder.clone(), title, private: spot.private, owner: spot.owner.clone() })
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        let Some(spot) = self.places.spot(address) else { return Vec::new() };
        let mut handles = vec![
            Handle::new("list", Level::See).titled("список"),
            Handle::new("stat", Level::See).titled("размер и время"),
            Handle::new("read", Level::Call).titled("читать"),
        ];
        if !spot.read_only {
            handles.push(Handle::new("write", Level::Edit).titled("класть и менять"));
            handles.push(Handle::new("mkdir", Level::Edit).titled("заводить папки"));
            handles.push(Handle::new("move", Level::Edit).titled("переносить"));
            handles.push(Handle::new("remove", Level::Edit).titled("удалять"));
        }
        for news in ["file.added", "file.changed", "file.removed"] {
            handles.push(Handle::new(news, Level::Listen).titled("новость папки"));
        }
        handles
    }

    /// An app owns its folder; a person their own folder.
    fn owns(&self, from: &Address, to: &Address) -> bool {
        self.places.spot(to).and_then(|spot| spot.owner).is_some_and(|owner| &owner == from)
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        // A move puts into another folder too: the bus checks that destination for the same caller and chain.
        if letter.handle == "move" {
            let to = letter.input.get("to").and_then(Value::as_str).and_then(|to| Address::parse(to).ok());
            if let (Some(to), Some(bus)) = (to, self.bus()) {
                match bus.decide(&letter.trail, &letter.from, &to, "write") {
                    Ok(decision) if decision.verdict == crate::bus::policy::Verdict::Allow => {}
                    Ok(_) | Err(_) => {
                        let to = to.to_string();
                        return Box::pin(async move {
                            Err(BusError::Denied(format!("положить в «{to}» нельзя без связи «правит»")))
                        });
                    }
                }
            }
        }
        let result = self.act(&letter);
        Box::pin(async move { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(station: bool) -> (Files, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), station);
        std::fs::create_dir_all(dir.path().join("content/music")).unwrap();
        (Files::new(places, Arc::new(|_| 1)), dir)
    }

    fn letter(to: &str, handle: &str, input: Value) -> Letter {
        Letter {
            id: 1,
            from: Address::app("paint"),
            to: Address::parse(to).unwrap(),
            handle: handle.into(),
            input,
            files: vec![],
            trail: crate::bus::Trail::app("paint"),
        }
    }

    async fn run(files: &Files, to: &str, handle: &str, input: Value) -> Result<Value, BusError> {
        files.call(letter(to, handle, input)).await
    }

    #[test]
    fn places_name_where_each_address_lives() {
        let (files, _dir) = made(false);
        let spot = files.places.spot(&Address::parse("files/home/Картинки/лето.png").unwrap()).unwrap();
        assert_eq!(spot.folder.as_str(), "files/home/Картинки");
        assert!(spot.path().ends_with("files/home/Картинки/лето.png"));
        let music = files.places.spot(&Address::parse("files/home/Музыка/a.mp3").unwrap()).unwrap();
        assert!(music.read_only && music.path().ends_with("content/music/a.mp3"));
        let person = files.places.spot(&Address::parse("files/person/u1/diary.txt").unwrap()).unwrap();
        assert!(person.private);
        assert_eq!(person.owner.unwrap().as_str(), "person/u1");
        assert!(files.places.spot(&Address::parse("files/home").unwrap()).is_none());
        assert!(files.owns(&Address::app("paint"), &Address::parse("files/app/paint/tmp/a.png").unwrap()));
        assert!(!files.owns(&Address::app("other"), &Address::parse("files/app/paint/a.png").unwrap()));
    }

    #[tokio::test]
    async fn an_app_writes_reads_lists_moves_and_removes_in_its_own_folder() {
        let (files, _dir) = made(false);
        run(&files, "files/app/paint/tmp/a.txt", "write", json!({"text": "привет"})).await.unwrap();
        let read = run(&files, "files/app/paint/tmp/a.txt", "read", json!({"as": "text"})).await.unwrap();
        assert_eq!(read["text"], "привет");
        let list = run(&files, "files/app/paint/tmp", "list", json!({})).await.unwrap();
        assert_eq!(list["items"][0]["name"], "a.txt");
        run(&files, "files/app/paint/tmp/a.txt", "move", json!({"to": "files/app/paint/done/a.txt"})).await.unwrap();
        assert!(run(&files, "files/app/paint/tmp/a.txt", "read", json!({})).await.is_err());
        let removed = run(&files, "files/app/paint/done/a.txt", "remove", json!({})).await.unwrap();
        assert_eq!(removed["bin"], false);
        // Music is the household's own and read only here.
        assert!(matches!(
            run(&files, "files/home/Музыка/x.mp3", "write", json!({"text": "x"})).await,
            Err(BusError::Denied(_))
        ));
    }

    #[tokio::test]
    async fn the_quota_and_the_stations_room_hold_and_the_households_files_go_to_the_bin() {
        let (files, _dir) = made(false);
        let big = B64.encode(vec![0u8; 1024 * 1024 + 10]);
        let over = run(&files, "files/app/paint/big.bin", "write", json!({"base64": big})).await;
        assert!(matches!(over, Err(BusError::Failed(ref why)) if why.contains("своя папка полна")), "{over:?}");
        run(&files, "files/home/Документы/a.txt", "write", json!({"text": "x"})).await.unwrap();
        let removed = run(&files, "files/home/Документы/a.txt", "remove", json!({})).await.unwrap();
        assert_eq!(removed["bin"], true);
        let (station, _dir) = made(true);
        let chunk = B64.encode(vec![0u8; 900 * 1024]);
        let mut refused = None;
        for index in 0..7 {
            let at = format!("files/home/Фото/{index}.bin");
            if let Err(error) = run(&station, &at, "write", json!({"base64": chunk})).await {
                refused = Some((index, error));
                break;
            }
        }
        let (index, error) = refused.expect("the station's room holds");
        assert_eq!(index, 5);
        assert!(matches!(error, BusError::Failed(ref why) if why.contains("на колонке")), "{error:?}");
    }

    #[test]
    fn the_sweep_cleans_old_tmp_files_only() {
        let (files, _dir) = made(false);
        let tmp = files.places.apps.join("paint/tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("fresh.txt"), "x").unwrap();
        assert_eq!(files.places.sweep(), 0);
        assert!(tmp.join("fresh.txt").exists());
    }
}
