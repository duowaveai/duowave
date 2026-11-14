//! The household's word on the home folder's folders (docs/plans/shared-folders.md, «Кто видит и что уходит из
//! дома»), and what a person attached to it («Подключено»). A row of the table `folders` per folder the household
//! said something of: who sees it, whether the model may read it and the voice name its files, whether it opens
//! outside the home; for an attached folder also where it is on this computer, how it came, and whether a drive is
//! known again by itself. A folder without a row keeps the defaults: seen by the admin and the family, nothing of it
//! to the model in the cloud or to Yandex's voice, closed outside the home.
//!
//! What is attached is pushed into `Places` (its address resolves there) and gets a link in the home folder's
//! «Подключено» while it is there — Finder shows it like a folder. Nothing is copied: what a person attached stays
//! where it is.
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::bus::address::Address;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::roles::Role;
use crate::storage::Db;
use crate::utils::time::now_iso;

use super::path;
use super::places::{AttachedPlace, Places, ATTACHED};

/// How an attached folder came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    /// A drive plugged in, and a person said «посмотреть».
    Drive,
    /// A folder dropped on the desk in the app's window.
    Dropped,
    /// A folder picked in the system's window, or its path typed.
    Picked,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::Drive => "drive",
            How::Dropped => "dropped",
            How::Picked => "picked",
        }
    }

    pub fn parse(text: &str) -> Option<How> {
        match text {
            "drive" => Some(How::Drive),
            "dropped" => Some(How::Dropped),
            "picked" => Some(How::Picked),
            _ => None,
        }
    }
}

/// The household's word on one folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub address: Address,
    /// An attached folder's place on this computer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// A drive's mark: its label and size, to know it again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<How>,
    /// A drive is attached again by itself when it comes back.
    pub remember: bool,
    /// Apps may put and change files in an attached folder.
    pub write: bool,
    /// Who sees it; the admin always.
    pub roles: Vec<Role>,
    /// The model may read its names and files though it is in the cloud.
    pub model: bool,
    /// The voice may name its files though Yandex says them.
    pub speak: bool,
    /// It opens outside the home.
    pub remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
}

impl Rule {
    /// The defaults of a folder nobody said anything of.
    pub fn defaults(address: Address) -> Rule {
        Rule {
            address,
            path: None,
            volume: None,
            how: None,
            remember: true,
            write: false,
            roles: vec![Role::Admin, Role::Member],
            model: false,
            speak: false,
            remote: false,
            added_at: None,
            added_by: None,
        }
    }

    /// Its name in «Подключено» for an attached one, its own name for a home folder.
    pub fn name(&self) -> String {
        self.address.segments().last().unwrap_or_default().to_string()
    }

    pub fn attached(&self) -> bool {
        self.path.is_some()
    }

    /// Whether a person of this role sees it.
    pub fn sees(&self, role: Role) -> bool {
        role == Role::Admin || self.roles.contains(&role)
    }
}

/// What a person changes of a folder: only what is given.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub roles: Option<Vec<String>>,
    pub write: Option<bool>,
    pub model: Option<bool>,
    pub speak: Option<bool>,
    pub remote: Option<bool>,
    pub remember: Option<bool>,
}

/// The table and its rows in memory.
pub struct Folders {
    db: Arc<Db>,
    places: Places,
    events: Option<EventBus>,
    rows: RwLock<Vec<Rule>>,
}

fn roles_text(roles: &[Role]) -> String {
    let mut words: Vec<&str> = roles.iter().map(|role| role.as_str()).collect();
    words.sort();
    words.dedup();
    words.join(",")
}

fn roles_of(text: &str) -> Vec<Role> {
    text.split(',').filter_map(|word| Role::parse(word.trim())).collect()
}

/// A name for «Подключено»: what the address takes (no `/`, `*`, `\`, control signs, no dot first), 64 letters.
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '*' | '\\') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned.trim_start_matches('.').trim().chars().take(64).collect::<String>().trim().to_string();
    if cleaned.is_empty() {
        "Папка".into()
    } else {
        cleaned
    }
}

impl Folders {
    pub fn new(db: Arc<Db>, places: Places, events: Option<EventBus>) -> Arc<Folders> {
        let folders = Arc::new(Folders { db, places, events, rows: RwLock::new(Vec::new()) });
        folders.load();
        folders
    }

    fn load(&self) {
        let read = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT address, path, volume, how, remember, write, roles, model, speak, remote, added_at, added_by
                 FROM folders ORDER BY address",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        let rows = match read {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(target: "bus", "files: the folders' table is not read: {error}");
                Vec::new()
            }
        };
        let rules = rows
            .into_iter()
            .filter_map(|(address, path, volume, how, remember, write, roles, model, speak, remote, at, by)| {
                Some(Rule {
                    address: Address::parse(&address).ok()?,
                    path: path.map(PathBuf::from),
                    volume,
                    how: how.as_deref().and_then(How::parse),
                    remember: remember != 0,
                    write: write != 0,
                    roles: roles_of(&roles),
                    model: model != 0,
                    speak: speak != 0,
                    remote: remote != 0,
                    added_at: Some(at),
                    added_by: by,
                })
            })
            .collect();
        *self.rows.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = rules;
    }

    fn save(&self, rule: &Rule) -> Result<(), String> {
        self.db
            .with(|c| {
                c.execute(
                    "INSERT INTO folders (address, path, volume, how, remember, write, roles, model, speak, remote,
                                          added_at, added_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(address) DO UPDATE SET path = ?2, volume = ?3, how = ?4, remember = ?5, write = ?6,
                         roles = ?7, model = ?8, speak = ?9, remote = ?10",
                    params![
                        rule.address.as_str(),
                        rule.path.as_ref().map(|path| path.to_string_lossy().to_string()),
                        rule.volume,
                        rule.how.map(How::as_str),
                        rule.remember as i64,
                        rule.write as i64,
                        roles_text(&rule.roles),
                        rule.model as i64,
                        rule.speak as i64,
                        rule.remote as i64,
                        rule.added_at.clone().unwrap_or_else(now_iso),
                        rule.added_by,
                    ],
                )
            })
            .map(|_| ())
            .map_err(|error| error.to_string())?;
        let mut rows = self.rows.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        rows.retain(|row| row.address != rule.address);
        rows.push(rule.clone());
        rows.sort_by(|a, b| a.address.as_str().cmp(b.address.as_str()));
        Ok(())
    }

    fn told(&self, what: &str) {
        if let Some(events) = &self.events {
            events.emit(GatewayEvent::FoldersChanged { what: what.into() });
        }
    }

    /// The word on a folder: its row, or the defaults.
    pub fn rule(&self, folder: &Address) -> Rule {
        self.rows
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|row| &row.address == folder)
            .cloned()
            .unwrap_or_else(|| Rule::defaults(folder.clone()))
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.rows.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// What is attached, here or not.
    pub fn attached(&self) -> Vec<Rule> {
        self.rules().into_iter().filter(Rule::attached).collect()
    }

    /// The attached drive with this mark.
    pub fn by_volume(&self, mark: &str) -> Option<Rule> {
        self.attached().into_iter().find(|rule| rule.volume.as_deref() == Some(mark))
    }

    /// The places and the links in «Подключено» in step with the table: a link for each attached folder that is
    /// there, none for one that is not or is gone.
    pub fn apply(&self) {
        let attached = self.attached();
        self.places.set_attached(
            attached
                .iter()
                .filter_map(|rule| {
                    Some(AttachedPlace { name: rule.name(), path: rule.path.clone()?, write: rule.write })
                })
                .collect(),
        );
        let links = self.places.home.join(ATTACHED);
        if self.places.station || !links.is_dir() {
            return;
        }
        for rule in &attached {
            let (Some(target), at) = (rule.path.as_ref(), links.join(rule.name())) else { continue };
            if target.is_dir() {
                self.places.link(target, &at);
            } else {
                self.places.unlink(&at);
            }
        }
        // A link the gateway made for something taken away since.
        for entry in std::fs::read_dir(&links).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !attached.iter().any(|rule| rule.name() == name) {
                self.places.unlink(&entry.path());
            }
        }
    }

    /// Whether a folder may be attached: an existing folder of this computer that is not the whole disk, a person's
    /// whole home, the gateway's data or the home folder itself — and none of these is inside it.
    fn allowed(&self, path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err(crate::tr!("нужен полный путь к папке"));
        }
        let real = path.canonicalize().map_err(|_| crate::tr!("папки {path} нет", path = path.display()))?;
        if !real.is_dir() {
            return Err(crate::tr!("{path} — не папка", path = path.display()));
        }
        let data = self.places.files.parent().map(Path::to_path_buf).unwrap_or_default();
        let guarded = [Some(data), Some(self.places.home.clone()), Some(self.places.apps.clone())]
            .into_iter()
            .flatten()
            .filter_map(|path| path.canonicalize().ok());
        for kept in guarded {
            if real.starts_with(&kept) || kept.starts_with(&real) {
                return Err(crate::tr!(
                    "эту папку подключить нельзя: в ней данные DuoWave или она сама в домашней папке"
                ));
            }
        }
        let person_home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
        if real.parent().is_none()
            || person_home.and_then(|home| home.canonicalize().ok()).is_some_and(|home| home == real)
        {
            return Err(crate::tr!("весь диск или всю папку пользователя подключить нельзя — выберите папку в ней"));
        }
        Ok(real)
    }

    /// A name in «Подключено» nobody has: «Отпуск», then «Отпуск 2».
    fn free_name(&self, wanted: &str) -> String {
        let taken: Vec<String> = self.attached().iter().map(Rule::name).collect();
        let base = clean_name(wanted);
        if !taken.contains(&base) {
            return base;
        }
        (2..).map(|n| format!("{base} {n}")).find(|name| !taken.contains(name)).unwrap_or(base)
    }

    /// Attaches a folder (or a drive) to «Подключено»; the same place attached again is the same row.
    pub fn attach(
        &self,
        path: &Path,
        name: Option<&str>,
        how: How,
        volume: Option<String>,
        by: Option<&str>,
    ) -> Result<Rule, String> {
        let real = self.allowed(path)?;
        if let Some(known) = self
            .attached()
            .into_iter()
            .find(|rule| rule.path.as_deref() == Some(real.as_path()) || (volume.is_some() && rule.volume == volume))
        {
            return Ok(known);
        }
        let wanted = name
            .map(str::to_string)
            .or_else(|| real.file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Папка".into());
        let name = self.free_name(&wanted);
        let address = Address::parse(&format!("files/home/{ATTACHED}/{name}"))
            .map_err(|error| crate::tr!("имя «{name}»: {error}", name = name, error = error))?;
        let rule = Rule {
            path: Some(real),
            volume,
            how: Some(how),
            added_at: Some(now_iso()),
            added_by: by.map(str::to_string),
            ..Rule::defaults(address)
        };
        self.save(&rule)?;
        self.apply();
        self.told("attached");
        Ok(rule)
    }

    /// Takes an attached folder away: its row, its link. The folder itself stays where it is.
    pub fn detach(&self, name: &str) -> Result<(), String> {
        let Some(rule) = self.attached().into_iter().find(|rule| rule.name() == name) else {
            return Err(crate::tr!("«{name}» не подключено", name = name));
        };
        self.db
            .with(|c| c.execute("DELETE FROM folders WHERE address = ?1", params![rule.address.as_str()]))
            .map_err(|error| error.to_string())?;
        self.rows.write().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|row| row.address != rule.address);
        self.places.unlink(&self.places.home.join(ATTACHED).join(name));
        self.apply();
        self.told("detached");
        Ok(())
    }

    /// A remembered drive came back at another place (another mount point, another letter).
    pub fn moved(&self, mark: &str, path: &Path) -> bool {
        let Some(mut rule) = self.by_volume(mark) else { return false };
        if rule.path.as_deref() == Some(path) {
            return false;
        }
        rule.path = Some(path.to_path_buf());
        self.save(&rule).is_ok()
    }

    /// A person's change of a folder's word; a home folder gets its row on its first change.
    pub fn change(&self, folder: &Address, change: &Change) -> Result<Rule, String> {
        if self.places.spot(folder).is_none_or(|spot| &spot.folder != folder) {
            return Err(crate::tr!("нет такой папки: {folder}", folder = folder));
        }
        let mut rule = self.rule(folder);
        if let Some(roles) = &change.roles {
            let mut parsed: Vec<Role> = roles.iter().filter_map(|word| Role::parse(word)).collect();
            if !parsed.contains(&Role::Admin) {
                parsed.push(Role::Admin);
            }
            rule.roles = parsed;
        }
        if let Some(write) = change.write {
            if !rule.attached() {
                return Err(crate::tr!("писать в папки дома можно всегда — по связям приложений"));
            }
            rule.write = write;
        }
        rule.model = change.model.unwrap_or(rule.model);
        rule.speak = change.speak.unwrap_or(rule.speak);
        rule.remote = change.remote.unwrap_or(rule.remote);
        rule.remember = change.remember.unwrap_or(rule.remember);
        if rule.added_at.is_none() {
            rule.added_at = Some(now_iso());
        }
        self.save(&rule)?;
        self.apply();
        self.told("rules");
        Ok(rule)
    }

    /// Whether an attached folder is there now.
    pub fn here(rule: &Rule) -> bool {
        rule.path.as_deref().is_some_and(|path| path::root(path).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made() -> (Arc<Folders>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), false)
            .with_home(Some(dir.path().join("DuoWave")));
        places.prepare();
        (Folders::new(Arc::new(Db::memory()), places, None), dir)
    }

    #[test]
    fn a_folder_without_a_word_keeps_the_defaults() {
        let (folders, _dir) = made();
        let rule = folders.rule(&Address::parse("files/home/Фото").unwrap());
        assert_eq!(rule.roles, vec![Role::Admin, Role::Member]);
        assert!(!rule.model && !rule.speak && !rule.remote);
        assert!(rule.sees(Role::Admin) && rule.sees(Role::Member) && !rule.sees(Role::Guest));
    }

    #[test]
    fn an_attached_folder_shows_in_podkliucheno_and_goes_without_touching_the_folder() {
        let (folders, dir) = made();
        let trip = dir.path().join("Pictures/Отпуск 2025");
        std::fs::create_dir_all(&trip).unwrap();
        std::fs::write(trip.join("1.jpg"), "x").unwrap();
        let rule = folders.attach(&trip, None, How::Dropped, None, Some("u1")).unwrap();
        assert_eq!(rule.address.as_str(), "files/home/Подключено/Отпуск 2025");
        assert!(!rule.write, "read only by default");
        let again = folders.attach(&trip, None, How::Picked, None, None).unwrap();
        assert_eq!(again.address, rule.address, "the same place attached again is the same row");
        let spot = folders.places.spot(&Address::parse("files/home/Подключено/Отпуск 2025/1.jpg").unwrap()).unwrap();
        assert!(spot.path().ends_with("Отпуск 2025/1.jpg"));
        #[cfg(unix)]
        assert!(dir.path().join("DuoWave/Подключено/Отпуск 2025/1.jpg").is_file(), "Finder sees it by the link");
        // Another folder of the same name gets its own.
        let other = dir.path().join("Other/Отпуск 2025");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(folders.attach(&other, None, How::Picked, None, None).unwrap().name(), "Отпуск 2025 2");
        folders.detach("Отпуск 2025").unwrap();
        assert!(trip.join("1.jpg").is_file(), "the folder itself stays");
        assert!(dir.path().join("DuoWave/Подключено/Отпуск 2025").symlink_metadata().is_err());
        assert!(folders.places.spot(&Address::parse("files/home/Подключено/Отпуск 2025/1.jpg").unwrap()).is_none());
        // The table holds it across a restart.
        let again = Folders::new(folders.db.clone(), folders.places.clone(), None);
        assert_eq!(again.attached().len(), 1);
    }

    #[test]
    fn the_gateways_own_places_are_never_attached() {
        let (folders, dir) = made();
        for path in [dir.path().join("data"), dir.path().join("DuoWave/Фото"), dir.path().to_path_buf()] {
            assert!(folders.attach(&path, None, How::Picked, None, None).is_err(), "{}", path.display());
        }
        assert!(folders.attach(Path::new("relative"), None, How::Picked, None, None).is_err());
        assert!(folders.attach(Path::new("/"), None, How::Picked, None, None).is_err());
    }

    #[test]
    fn a_change_keeps_the_admin_and_writes_only_attached_ones() {
        let (folders, _dir) = made();
        let photos = Address::parse("files/home/Фото").unwrap();
        let change =
            Change { roles: Some(vec!["member".into(), "guest".into()]), model: Some(true), ..Change::default() };
        let rule = folders.change(&photos, &change).unwrap();
        assert!(rule.sees(Role::Guest) && rule.roles.contains(&Role::Admin) && rule.model);
        assert!(folders.change(&photos, &Change { write: Some(true), ..Change::default() }).is_err());
        assert!(folders.change(&Address::parse("files/home/Фото/Отпуск").unwrap(), &change).is_err());
    }

    #[test]
    fn names_are_cleaned_for_the_address() {
        assert_eq!(clean_name("  .Отпуск/2025*  "), "Отпуск 2025");
        assert_eq!(clean_name("..."), "Папка");
        assert_eq!(clean_name(&"я".repeat(100)).chars().count(), 64);
    }
}
