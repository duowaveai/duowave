//! The station's settings, kept on the computer (docs/plans/station-placement.md, section 10): the computer takes a
//! copy whenever the station names a new revision, and before a move or an update; the last ten stay, sealed with the
//! pair's key as they came. After a reset of the station — its settings back with one button.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// How many copies stay.
pub const KEPT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub id: String,
    pub at: String,
    pub revision: u64,
    pub size: u64,
}

pub struct Backups {
    dir: Option<PathBuf>,
}

impl Backups {
    /// In the state folder, beside the database; none for a gateway in memory (tests that do not ask for one).
    pub fn new(state_dir: Option<&Path>) -> Backups {
        Backups { dir: state_dir.map(|dir| dir.join("station-backups")) }
    }

    /// One station's copies: a folder of its own under the common one.
    pub fn for_station(state_dir: Option<&Path>, key: &str) -> Backups {
        let folder: String =
            key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
        Backups { dir: state_dir.map(|dir| dir.join("station-backups").join(folder)) }
    }

    /// The copies kept before each station had its folder (the common folder's own files) move into this station's.
    pub fn adopt_common(&self, state_dir: Option<&Path>) {
        let (Some(common), Some(mine)) = (state_dir.map(|dir| dir.join("station-backups")), self.dir.as_deref()) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&common) else { return };
        let old: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|extension| extension == "json"))
            .collect();
        if old.is_empty() || std::fs::create_dir_all(mine).is_err() {
            return;
        }
        for path in old {
            if let Some(name) = path.file_name() {
                let _ = std::fs::rename(&path, mine.join(name));
            }
        }
    }

    fn dir(&self) -> Result<&Path, String> {
        self.dir.as_deref().ok_or_else(|| "копии настроек некуда сохранить: у шлюза нет папки данных".to_string())
    }

    /// Keeps a sealed package and lets the oldest beyond ten go.
    pub fn save(&self, sealed: &Value, revision: u64) -> Result<BackupEntry, String> {
        let dir = self.dir()?;
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let now = chrono::Utc::now();
        let id = format!("{}-r{revision}", now.format("%Y%m%d-%H%M%S"));
        let path = dir.join(format!("{id}.json"));
        let text = serde_json::to_vec(sealed).map_err(|error| error.to_string())?;
        super::station_fs::write_atomic(&path, &text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        for old in self.list().into_iter().skip(KEPT) {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", old.id)));
        }
        Ok(BackupEntry {
            id,
            at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            revision,
            size: text.len() as u64,
        })
    }

    /// The copies, newest first.
    pub fn list(&self) -> Vec<BackupEntry> {
        let Some(dir) = self.dir.as_deref() else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut list: Vec<BackupEntry> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?.to_string();
                let (stamp, revision) = id.split_once("-r")?;
                let at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").ok()?.and_utc();
                Some(BackupEntry {
                    at: at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    revision: revision.parse().ok()?,
                    size: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                    id,
                })
            })
            .collect();
        list.sort_by(|a, b| b.id.cmp(&a.id));
        list
    }

    pub fn latest(&self) -> Option<BackupEntry> {
        self.list().into_iter().next()
    }

    /// A copy, sealed as it was kept.
    pub fn read(&self, id: &str) -> Result<Value, String> {
        if !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
            return Err("нет такой копии".into());
        }
        let path = self.dir()?.join(format!("{id}.json"));
        let text = std::fs::read(&path).map_err(|_| "нет такой копии".to_string())?;
        serde_json::from_slice(&text).map_err(|_| "копия повреждена".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_ten_newest_first_and_reads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let backups = Backups::new(Some(dir.path()));
        for revision in 1..=12u64 {
            // Each a second apart in name, as the computer would take them.
            let id = format!("2026110{}-0000{:02}-r{revision}", 1 + revision / 10, revision);
            let path = dir.path().join("station-backups");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(format!("{id}.json")), b"{}").unwrap();
        }
        let saved = backups.save(&serde_json::json!({"sealed": 1}), 13).unwrap();
        let list = backups.list();
        assert_eq!(list.len(), KEPT);
        assert_eq!(list[0].id, saved.id, "the newest first");
        assert_eq!(list[0].revision, 13);
        assert_eq!(backups.read(&saved.id).unwrap(), serde_json::json!({"sealed": 1}));
        assert!(backups.read("../../etc/passwd").is_err());
        assert!(Backups::new(None).save(&Value::Null, 1).is_err());
    }
}
