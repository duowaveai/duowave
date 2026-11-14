//! The places of the folders on this gateway (docs/plans/shared-folders.md, «Домашняя папка — видимая»). The home
//! folder is one ordinary folder of this computer — `~/DuoWave` in the app (HOME_FILES), `DATA_DIR/files/home`
//! elsewhere and on the station — and for apps, the agent and the voice it is the whole disk:
//!
//! ```text
//! ~/DuoWave/
//! ├── Фото/ Документы/ Видео/ Загрузки/ Картинки/   files/home/<папка>
//! ├── Музыка/ Сказки/                                files/home/Музыка — links to CONTENT_DIR's music and tales
//! ├── Проекты/<id>/                                  files/project/<id>
//! └── Подключено/<имя>                               files/home/Подключено/<имя> — a link to what a person attached
//! ```
//!
//! An app's own folder (`files/app/<id>`) and a person's (`files/person/<id>`) stay in DATA_DIR: they are not for a
//! person's hands. The music and the tales stay in CONTENT_DIR, where the player and the station take them from; the
//! home folder shows them by links the gateway makes. What a person attached stays where it is too: the gateway keeps
//! a link to it in «Подключено» while it is there.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use crate::bus::address::{self, Address};

use super::path;

/// The household's folders made at the start.
pub const HOME_FOLDERS: &[&str] = &["Фото", "Документы", "Видео", "Загрузки", "Картинки"];
/// The household's music and tales: CONTENT_DIR's folders, read only here.
pub const CONTENT_FOLDERS: &[(&str, &str)] = &[("Музыка", "music"), ("Сказки", "tales")];
/// What a person attached: a drive, a folder dropped on the desk or picked.
pub const ATTACHED: &str = "Подключено";
/// The projects' folders: `files/project/<id>`.
pub const PROJECTS: &str = "Проекты";
/// Where removed and replaced files wait their week, in each place that has a bin.
pub const BIN: &str = ".bin";
/// How long a file waits in an app's `tmp/` before it is cleaned away.
pub const TMP_TTL: Duration = Duration::from_secs(24 * 3600);
/// How long a removed or replaced file waits in the bin.
pub const BIN_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// What kind of place an address is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An app's own folder.
    App,
    /// A folder of the home folder: Фото, Документы…
    Home,
    /// The household's music or tales.
    Content,
    /// «Подключено» itself: the list of what is attached.
    AttachedList,
    /// Something a person attached.
    Attached,
    /// A project's folder.
    Project,
    /// A person's own folder.
    Person,
}

/// Something a person attached, as the folders' table keeps it (rules.rs).
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedPlace {
    pub name: String,
    pub path: PathBuf,
    /// Apps may put and change files there (the person's tick); read only otherwise.
    pub write: bool,
}

/// Where the bytes of an address live, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    /// The folder participant: `files/home/Картинки`, `files/app/<id>`, `files/home/Подключено/<имя>`.
    pub folder: Address,
    pub kind: Kind,
    /// Its directory on this disk.
    pub root: PathBuf,
    /// The path below it.
    pub rest: Vec<String>,
    pub read_only: bool,
    pub private: bool,
    pub owner: Option<Address>,
    /// Where its removed and replaced files wait; none for an app's own folder (they go at once).
    pub bin: Option<PathBuf>,
}

impl Spot {
    /// The plain path, unchecked: for the gateway's own work. Letters go through `path::existing` and `writable`.
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
    /// `DATA_DIR/files`: people's folders, and the home folder where none is set.
    pub files: PathBuf,
    /// The home folder: HOME_FILES (the app's `~/DuoWave`), `DATA_DIR/files/home` without it.
    pub home: PathBuf,
    /// `DATA_DIR/plugin-files`: the apps' own folders, their pictures at the top.
    pub apps: PathBuf,
    /// CONTENT_DIR.
    pub content: PathBuf,
    /// The station's limits.
    pub station: bool,
    /// What a person attached, kept in step by the folders' table.
    attached: Arc<RwLock<Vec<AttachedPlace>>>,
    /// What the start could not do with the old places, for the panel.
    notes: Arc<Mutex<Vec<String>>>,
}

impl Places {
    pub fn new(data_dir: &Path, content: &Path, station: bool) -> Places {
        let files = data_dir.join("files");
        Places {
            home: files.join("home"),
            files,
            apps: data_dir.join("plugin-files"),
            content: content.to_path_buf(),
            station,
            attached: Arc::new(RwLock::new(Vec::new())),
            notes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The home folder somewhere else: the app's `~/DuoWave`. The station keeps it in its data.
    pub fn with_home(mut self, home: Option<PathBuf>) -> Places {
        if let Some(home) = home.filter(|_| !self.station) {
            self.home = home;
        }
        self
    }

    /// Whether the home folder is a person's own folder (not inside the gateway's data).
    pub fn visible(&self) -> bool {
        self.home != self.files.join("home")
    }

    /// What is attached now, from the folders' table.
    pub fn set_attached(&self, attached: Vec<AttachedPlace>) {
        *self.attached.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = attached;
    }

    pub fn attached(&self) -> Vec<AttachedPlace> {
        self.attached.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn note(&self, line: String) {
        tracing::warn!(target: "bus", "files: {line}");
        let mut notes = self.notes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !notes.contains(&line) {
            notes.push(line);
        }
    }

    pub fn notes(&self) -> Vec<String> {
        self.notes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The projects' folder.
    pub fn projects(&self) -> PathBuf {
        self.home.join(PROJECTS)
    }

    /// The bin of the home folder: the household's files, the projects', the attached ones'.
    pub fn home_bin(&self) -> PathBuf {
        self.home.join(BIN)
    }

    /// Where an address lives; none for an address no folder holds.
    pub fn spot(&self, address: &Address) -> Option<Spot> {
        if address.kind() != address::FILES {
            return None;
        }
        let parts: Vec<&str> = address.segments().collect();
        let (area, name) = (*parts.first()?, *parts.get(1)?);
        let rest = |from: usize| parts.get(from..).unwrap_or_default().iter().map(|part| part.to_string()).collect();
        let folder = |path: &str| Address::parse(&format!("files/{path}")).ok();
        let home_bin = Some(self.home_bin());
        let spot = match area {
            // An app's own folder is where its pictures always were (plugins/files.rs): they are its top.
            "app" => Spot {
                folder: folder(&format!("app/{name}"))?,
                kind: Kind::App,
                root: self.apps.join(name),
                rest: rest(2),
                read_only: false,
                private: false,
                owner: Some(Address::app(name)),
                bin: None,
            },
            "home" => match (CONTENT_FOLDERS.iter().find(|(title, _)| *title == name), name) {
                (Some((_, folder_name)), _) => Spot {
                    folder: folder(&format!("home/{name}"))?,
                    kind: Kind::Content,
                    root: self.content.join(folder_name),
                    rest: rest(2),
                    read_only: true,
                    private: false,
                    owner: None,
                    bin: None,
                },
                (None, ATTACHED) => match parts.get(2) {
                    None => Spot {
                        folder: folder(&format!("home/{ATTACHED}"))?,
                        kind: Kind::AttachedList,
                        root: self.home.join(ATTACHED),
                        rest: Vec::new(),
                        read_only: true,
                        private: false,
                        owner: None,
                        bin: None,
                    },
                    Some(item) => {
                        let attached = self.attached().into_iter().find(|attached| attached.name == *item)?;
                        Spot {
                            folder: folder(&format!("home/{ATTACHED}/{item}"))?,
                            kind: Kind::Attached,
                            root: attached.path,
                            rest: rest(3),
                            read_only: !attached.write,
                            private: false,
                            owner: None,
                            bin: home_bin,
                        }
                    }
                },
                // The projects are `files/project/<id>`: one address for one place.
                (None, PROJECTS) => return None,
                (None, _) if path::hidden(name) => return None,
                (None, _) => Spot {
                    folder: folder(&format!("home/{name}"))?,
                    kind: Kind::Home,
                    root: self.home.join(name),
                    rest: rest(2),
                    read_only: false,
                    private: false,
                    owner: None,
                    bin: home_bin,
                },
            },
            "person" => Spot {
                folder: folder(&format!("person/{name}"))?,
                kind: Kind::Person,
                root: self.files.join("person").join(name),
                rest: rest(2),
                read_only: false,
                private: true,
                owner: Address::new(address::PERSON, name).ok(),
                bin: Some(self.files.join(BIN)),
            },
            "project" => Spot {
                folder: folder(&format!("project/{name}"))?,
                kind: Kind::Project,
                root: self.projects().join(name),
                rest: rest(2),
                read_only: false,
                private: false,
                owner: None,
                bin: home_bin,
            },
            _ => return None,
        };
        Some(spot)
    }

    /// The home folder made ready: its folders, «Подключено», «Проекты», the links to the music and the tales, and
    /// what lay in the old places moved in (once, when the home folder became a person's own).
    pub fn prepare(&self) {
        for name in HOME_FOLDERS.iter().copied().chain([ATTACHED, PROJECTS]) {
            if let Err(error) = std::fs::create_dir_all(self.home.join(name)) {
                self.note(crate::tr!(
                    "не создать папку «{name}» в {home}: {error}",
                    name = name,
                    home = self.home.display(),
                    error = error
                ));
            }
        }
        if self.visible() {
            self.move_old();
            for (title, folder) in CONTENT_FOLDERS {
                let target = self.content.join(folder);
                let _ = std::fs::create_dir_all(&target);
                self.link(&target, &self.home.join(title));
            }
        } else {
            // The projects lived in `DATA_DIR/files/project` before the home folder had «Проекты».
            self.move_into(&self.files.join("project"), &self.projects());
        }
    }

    /// What lay in the old places of the household's folders and projects goes into the home folder: moved, never
    /// copied; what is there already by that name, or cannot move (another disk), stays and is said in the panel.
    fn move_old(&self) {
        let old = self.files.join("home");
        for entry in std::fs::read_dir(&old).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if path::hidden(&name) || !entry.path().is_dir() {
                continue;
            }
            self.move_into(&entry.path(), &self.home.join(&name));
        }
        self.move_into(&self.files.join("project"), &self.projects());
        for gone in [old, self.files.join("project")] {
            remove_if_empty(&gone);
        }
    }

    /// Moves each entry of `from` into `to` unless one of that name is there; `from` goes when it is left empty.
    fn move_into(&self, from: &Path, to: &Path) {
        let Ok(entries) = std::fs::read_dir(from) else { return };
        let _ = std::fs::create_dir_all(to);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let target = to.join(&name);
            if target.symlink_metadata().is_ok() {
                self.note(crate::tr!(
                    "«{name}» осталось в {from}: в {to} уже есть такое",
                    name = name.to_string_lossy(),
                    from = from.display(),
                    to = to.display()
                ));
                continue;
            }
            match std::fs::rename(entry.path(), &target) {
                Ok(()) => {
                    tracing::info!(target: "bus", "files: {} moved into {}", entry.path().display(), to.display())
                }
                Err(error) => self.note(crate::tr!(
                    "«{name}» не перенеслось из {from} в {to}: {error}",
                    name = name.to_string_lossy(),
                    from = from.display(),
                    to = to.display(),
                    error = error
                )),
            }
        }
        remove_if_empty(from);
    }

    /// A link the gateway keeps in the home folder (Музыка, Сказки, what is attached): made where there is none, set
    /// right where it points elsewhere; a real folder of that name is the person's and stays.
    pub fn link(&self, target: &Path, at: &Path) {
        match at.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                if std::fs::read_link(at).is_ok_and(|now| now == target) {
                    return;
                }
                let _ = std::fs::remove_file(at);
            }
            Ok(_) => return,
            Err(_) => {}
        }
        if let Err(error) = make_link(target, at) {
            tracing::info!(target: "bus", "files: no link {} → {}: {error}", at.display(), target.display());
        }
    }

    /// Takes away a link the gateway made; anything else there stays.
    pub fn unlink(&self, at: &Path) {
        if at.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
            let _ = std::fs::remove_file(at);
        }
    }

    /// The bytes of every file below a directory, links not followed.
    pub fn size_of(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|entry| match entry.path().symlink_metadata() {
                Ok(meta) if meta.is_dir() => Places::size_of(&entry.path()),
                Ok(meta) if meta.file_type().is_symlink() => 0,
                Ok(meta) => meta.len(),
                Err(_) => 0,
            })
            .sum()
    }

    /// Cleans every app's `tmp/` of what is older than its time, and the bins of what waited its week.
    pub fn sweep(&self) -> usize {
        let mut removed = 0;
        let now = SystemTime::now();
        let old = |path: &Path, ttl: Duration| {
            path.symlink_metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > ttl)
        };
        if let Ok(apps) = std::fs::read_dir(&self.apps) {
            for app in apps.flatten() {
                let tmp = app.path().join("tmp");
                for entry in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
                    if old(&entry.path(), TMP_TTL) && path::remove_any(&entry.path()).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        for bin in [self.files.join(BIN), self.home_bin()] {
            for entry in std::fs::read_dir(&bin).into_iter().flatten().flatten() {
                if old(&entry.path(), BIN_TTL) && path::remove_any(&entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
}

fn remove_if_empty(dir: &Path) {
    if std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none()) {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(unix)]
fn make_link(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

/// Windows lets a program make links only with the developer mode on; without, the folder has no link and the panel
/// shows where the thing is.
#[cfg(windows)]
fn make_link(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, at)
}

#[cfg(not(any(unix, windows)))]
fn make_link(_target: &Path, _at: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("no links here"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(dir: &Path, home: Option<PathBuf>) -> Places {
        Places::new(&dir.join("data"), &dir.join("content"), false).with_home(home)
    }

    #[test]
    fn each_address_has_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let places = places(dir.path(), Some(dir.path().join("DuoWave")));
        let spot = |text: &str| places.spot(&Address::parse(text).unwrap());
        let photo = spot("files/home/Фото/лето.jpg").unwrap();
        assert_eq!((photo.folder.as_str(), photo.kind), ("files/home/Фото", Kind::Home));
        assert!(photo.path().ends_with("DuoWave/Фото/лето.jpg"));
        assert_eq!(photo.bin, Some(dir.path().join("DuoWave/.bin")));
        let project = spot("files/project/сайт/index.html").unwrap();
        assert!(project.path().ends_with("DuoWave/Проекты/сайт/index.html"));
        let music = spot("files/home/Музыка/a.mp3").unwrap();
        assert!(music.read_only && music.path().ends_with("content/music/a.mp3"));
        assert!(spot("files/home/Проекты/сайт").is_none(), "one address for one place");
        assert!(spot("files/home/.bin/x").is_none());
        assert_eq!(spot("files/home/Подключено").unwrap().kind, Kind::AttachedList);
        assert!(spot("files/home/Подключено/Флешка/a.jpg").is_none(), "nothing attached yet");
        places.set_attached(vec![AttachedPlace {
            name: "Флешка".into(), path: dir.path().join("usb"), write: false
        }]);
        let usb = spot("files/home/Подключено/Флешка/DCIM/a.jpg").unwrap();
        assert_eq!(usb.folder.as_str(), "files/home/Подключено/Флешка");
        assert_eq!(
            (usb.kind, usb.read_only, usb.rest.clone()),
            (Kind::Attached, true, vec!["DCIM".to_string(), "a.jpg".to_string()])
        );
        assert!(usb.path().ends_with("usb/DCIM/a.jpg"));
        let app = spot("files/app/paint/tmp/a.png").unwrap();
        assert_eq!((app.kind, app.bin.clone()), (Kind::App, None));
        assert!(app.path().ends_with("data/plugin-files/paint/tmp/a.png"));
        assert!(spot("files/person/u1/diary.txt").unwrap().private);
    }

    #[test]
    fn the_station_keeps_its_home_in_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let station = Places::new(&dir.path().join("data"), &dir.path().join("content"), true)
            .with_home(Some(dir.path().join("DuoWave")));
        assert!(!station.visible());
        assert!(station.home.ends_with("data/files/home"));
    }

    #[test]
    fn the_start_makes_the_home_folder_and_moves_the_old_one_in() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("data/files/home/Фото");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("старое.jpg"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("data/files/project/сайт")).unwrap();
        std::fs::create_dir_all(dir.path().join("content/music")).unwrap();
        std::fs::write(dir.path().join("content/music/песня.mp3"), "x").unwrap();
        let home = dir.path().join("DuoWave");
        std::fs::create_dir_all(home.join("Документы")).unwrap();
        let places = places(dir.path(), Some(home.clone()));
        places.prepare();
        for name in HOME_FOLDERS.iter().chain([&ATTACHED, &PROJECTS]) {
            assert!(home.join(name).is_dir(), "{name}");
        }
        assert!(home.join("Фото/старое.jpg").is_file());
        assert!(home.join("Проекты/сайт").is_dir());
        assert!(!dir.path().join("data/files/home").exists(), "the old place is gone once empty");
        #[cfg(unix)]
        {
            assert!(home.join("Музыка").symlink_metadata().unwrap().file_type().is_symlink());
            assert!(home.join("Музыка/песня.mp3").is_file(), "the music shows in the home folder by a link");
        }
        // A second start finds all in place.
        places.prepare();
        assert!(places.notes().is_empty(), "{:?}", places.notes());
    }

    #[test]
    fn a_name_taken_in_the_home_folder_keeps_the_old_one_where_it_was() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data/files/home/Фото/Отпуск")).unwrap();
        let home = dir.path().join("DuoWave");
        std::fs::create_dir_all(home.join("Фото/Отпуск")).unwrap();
        let places = places(dir.path(), Some(home));
        places.prepare();
        assert!(dir.path().join("data/files/home/Фото/Отпуск").is_dir());
        assert_eq!(places.notes().len(), 1, "{:?}", places.notes());
    }
}
