//! The plugins' pictures (docs/plans/plugin-schedules.md, stage 10): a plugin with the right `files` keeps pictures it
//! downloaded (`host.files.download`), the panel shows them with its own key (`useFile`), and they go when the plugin
//! does. The gateway downloads them itself into the plugin's folder, never through the plugin's memory, and checks the
//! type by the file's first bytes, not by the link.
//!
//! Only pictures — PNG, JPEG, WebP, GIF. On a computer a picture is up to 10 MB, all of a plugin's within its
//! `budget.storageMb`. On the station room is short (`/data` some 45 MB free, measured 11.11.2026), so it has a limit
//! of its own, the owner's decision 10 (12.11.2026): a picture up to 1 MB, all the plugins' pictures together up to
//! 5 MB. A plugin with `prefer: home` keeps its pictures on the computer: its calls run there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

const MB: u64 = 1024 * 1024;
/// A picture at most, on a computer.
pub const FILE_MAX: u64 = 10 * MB;
/// On the station: a picture, and all the plugins' pictures together (the owner's decision 10).
pub const STATION_FILE_MAX: u64 = MB;
pub const STATION_ALL_MAX: u64 = 5 * MB;
const NAME_MAX: usize = 64;

/// A picture's type by its first bytes: its extension and media type.
pub fn picture_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(("png", "image/png"))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else {
        None
    }
}

fn media_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// A plugin's name for a picture, safe on any disk: lower-case Latin letters, digits, `-`, `_` and `.`, no path, with
/// the extension of its type.
pub fn picture_name(name: &str, extension: &str) -> Result<String, String> {
    let name = name.trim().to_ascii_lowercase();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
    let stem = if stem.is_empty() { "picture" } else { stem };
    let safe = stem.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if !safe || stem.starts_with('-') || stem.len() + extension.len() + 1 > NAME_MAX {
        return Err(format!("имя «{name}» — латиница, цифры, «-» и «_», до {NAME_MAX} знаков"));
    }
    Ok(format!("{stem}.{extension}"))
}

/// The folder of the plugins' pictures: DATA_DIR/plugin-files, data/plugin-files without it.
pub fn root_of(data_dir: &Option<PathBuf>) -> PathBuf {
    match data_dir {
        Some(root) => root.join("plugin-files"),
        None => PathBuf::from("data/plugin-files"),
    }
}

/// Where the plugins' pictures live, and the limits of where this gateway runs.
pub struct PluginFiles {
    root: PathBuf,
    /// This gateway runs on the station: its limits.
    on_station: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl PluginFiles {
    pub fn new(root: PathBuf, on_station: Arc<dyn Fn() -> bool + Send + Sync>) -> PluginFiles {
        PluginFiles { root, on_station }
    }

    fn folder(&self, plugin: &str) -> PathBuf {
        self.root.join(plugin)
    }

    /// A plugin's pictures: name, size, when written.
    fn files(&self, plugin: &str) -> Vec<(String, u64, i64)> {
        let Ok(entries) = std::fs::read_dir(self.folder(plugin)) else { return Vec::new() };
        let mut files: Vec<(String, u64, i64)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let written = meta
                    .modified()
                    .ok()
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |at| at.as_millis() as i64);
                (!name.starts_with('.')).then_some((name, meta.len(), written))
            })
            .collect();
        files.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        files
    }

    /// The bytes of one plugin's pictures, or of all the plugins'.
    pub fn usage(&self, plugin: Option<&str>) -> u64 {
        match plugin {
            Some(plugin) => self.files(plugin).iter().map(|(_, size, _)| size).sum(),
            None => std::fs::read_dir(&self.root)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| entry.path().is_dir())
                        .map(|entry| self.usage(Some(&entry.file_name().to_string_lossy())))
                        .sum()
                })
                .unwrap_or(0),
        }
    }

    /// A downloaded picture kept for the plugin: checked, within the limits, written whole or not at all.
    pub fn put(&self, plugin: &str, name: &str, bytes: &[u8], storage_mb: u64) -> Result<Value, String> {
        let (extension, kind) = picture_kind(bytes).ok_or("это не картинка: годятся PNG, JPEG, WebP и GIF")?;
        let name = picture_name(name, extension)?;
        let size = bytes.len() as u64;
        let replaced = self.files(plugin).iter().find(|(known, _, _)| *known == name).map_or(0, |(_, size, _)| *size);
        if (self.on_station)() {
            if size > STATION_FILE_MAX {
                return Err("на колонке нет места для картинки: до 1 МБ на картинку".into());
            }
            if self.usage(None) - replaced + size > STATION_ALL_MAX {
                return Err("на колонке нет места для картинок: все картинки приложений — до 5 МБ".into());
            }
        } else if size > FILE_MAX {
            return Err("картинка больше 10 МБ".into());
        }
        if self.usage(Some(plugin)) - replaced + size > storage_mb * MB {
            return Err(format!("места приложения ({storage_mb} МБ) не хватает: удалите старые картинки"));
        }
        let folder = self.folder(plugin);
        std::fs::create_dir_all(&folder).map_err(|error| format!("папка картинок: {error}"))?;
        let part = folder.join(format!(".{name}.part"));
        std::fs::write(&part, bytes).map_err(|error| format!("картинка не записалась: {error}"))?;
        std::fs::rename(&part, folder.join(&name)).map_err(|error| format!("картинка не записалась: {error}"))?;
        Ok(json!({ "name": name, "size": size, "type": kind }))
    }

    /// `host.files.list()`: the newest first.
    pub fn list(&self, plugin: &str) -> Vec<Value> {
        self.files(plugin)
            .into_iter()
            .map(|(name, size, written)| json!({ "name": name, "size": size, "type": media_type(&name), "written": written }))
            .collect()
    }

    /// `host.files.delete(name)`: whether there was one.
    pub fn delete(&self, plugin: &str, name: &str) -> bool {
        self.files(plugin).iter().any(|(known, _, _)| known == name)
            && std::fs::remove_file(self.folder(plugin).join(name)).is_ok()
    }

    /// A picture for the panel: its bytes and media type.
    pub fn read(&self, plugin: &str, name: &str) -> Option<(Vec<u8>, &'static str)> {
        if !self.files(plugin).iter().any(|(known, _, _)| known == name) {
            return None;
        }
        std::fs::read(self.folder(plugin).join(name)).ok().map(|bytes| (bytes, media_type(name)))
    }

    /// The plugin's pictures replaced whole by those the history kept (history/data.rs): within the station's limits,
    /// written beside the old ones first and swapped in, so a failure leaves the old ones as they were.
    pub fn replace(&self, plugin: &str, pictures: &[(String, Vec<u8>)]) -> Result<(), String> {
        let plain = |name: &str| {
            !name.is_empty() && !name.starts_with('.') && name.len() <= NAME_MAX + 5 && !name.contains(['/', '\\'])
        };
        if let Some((name, _)) = pictures.iter().find(|(name, _)| !plain(name)) {
            return Err(format!("имя картинки «{name}» не годится"));
        }
        let total: u64 = pictures.iter().map(|(_, bytes)| bytes.len() as u64).sum();
        if (self.on_station)() {
            if let Some((name, _)) = pictures.iter().find(|(_, bytes)| bytes.len() as u64 > STATION_FILE_MAX) {
                return Err(format!("картинка {name} больше 1 МБ — на колонке такие не хранятся"));
            }
            let others = self.usage(None).saturating_sub(self.usage(Some(plugin)));
            if others + total > STATION_ALL_MAX {
                return Err(format!(
                    "картинки не помещаются на колонке: у всех плагинов вместе до {} МБ",
                    STATION_ALL_MAX / MB
                ));
            }
        }
        let folder = self.folder(plugin);
        if !folder.starts_with(&self.root) || folder == self.root {
            return Err("не та папка".into());
        }
        let fresh = self.root.join(format!(".{plugin}.restore"));
        let _ = std::fs::remove_dir_all(&fresh);
        std::fs::create_dir_all(&fresh).map_err(|error| format!("картинки не записались: {error}"))?;
        for (name, bytes) in pictures {
            if let Err(error) = std::fs::write(fresh.join(name), bytes) {
                let _ = std::fs::remove_dir_all(&fresh);
                return Err(format!("картинка {name} не записалась: {error}"));
            }
        }
        let old = self.root.join(format!(".{plugin}.old"));
        let _ = std::fs::remove_dir_all(&old);
        if folder.exists() {
            std::fs::rename(&folder, &old).map_err(|error| format!("картинки не заменились: {error}"))?;
        }
        if let Err(error) = std::fs::rename(&fresh, &folder) {
            let _ = std::fs::rename(&old, &folder);
            return Err(format!("картинки не заменились: {error}"));
        }
        let _ = std::fs::remove_dir_all(&old);
        if pictures.is_empty() {
            let _ = std::fs::remove_dir(&folder);
        }
        Ok(())
    }

    /// The plugin went: its pictures go with it.
    pub fn forget(&self, plugin: &str) {
        let folder = self.folder(plugin);
        if folder.starts_with(&self.root) && folder != self.root {
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    /// For the plugin's card: its pictures, their bytes, and the limit they count against here.
    pub fn view(&self, plugin: &str, storage_mb: u64) -> Value {
        let on_station = (self.on_station)();
        json!({
            "files": self.list(plugin),
            "bytes": self.usage(Some(plugin)),
            "limitBytes": if on_station { STATION_ALL_MAX } else { storage_mb * MB },
            "onStation": on_station,
        })
    }

    /// For «Ресурсы колонки»: every plugin's pictures together, and the station's limit.
    pub fn summary(&self) -> Value {
        let on_station = (self.on_station)();
        json!({ "bytes": self.usage(None), "limitBytes": on_station.then_some(STATION_ALL_MAX), "onStation": on_station })
    }

    /// The root, for the move (placement/transfer.rs).
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn png(size: usize) -> Vec<u8> {
        let mut bytes = PNG.to_vec();
        bytes.resize(size, 0);
        bytes
    }

    #[test]
    fn knows_a_picture_by_its_bytes_and_keeps_names_safe() {
        assert_eq!(picture_kind(PNG), Some(("png", "image/png")));
        assert_eq!(picture_kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(("jpg", "image/jpeg")));
        assert_eq!(picture_kind(b"GIF89a...."), Some(("gif", "image/gif")));
        assert_eq!(picture_kind(b"RIFF\0\0\0\0WEBPVP8 "), Some(("webp", "image/webp")));
        assert_eq!(picture_kind(b"<html>"), None);
        assert_eq!(picture_name("Day.PNG", "png").unwrap(), "day.png");
        assert_eq!(picture_name("day", "jpg").unwrap(), "day.jpg", "the extension of its type");
        assert_eq!(picture_name("", "gif").unwrap(), "picture.gif");
        assert!(picture_name("../etc/passwd", "png").is_err());
        assert!(picture_name("кот", "png").is_err());
    }

    #[test]
    fn keeps_a_plugins_pictures_within_the_limits_of_where_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let station = Arc::new(AtomicBool::new(false));
        let on_station = station.clone();
        let files =
            PluginFiles::new(dir.path().join("plugin-files"), Arc::new(move || on_station.load(Ordering::Relaxed)));
        // A computer: up to 10 MB a picture, within the plugin's room.
        assert_eq!(files.put("day", "today", &png(2 * MB as usize), 4).unwrap()["name"], "today.png");
        assert!(files.put("day", "huge", &png(11 * MB as usize), 64).unwrap_err().contains("больше 10 МБ"));
        assert!(files.put("day", "more", &png(3 * MB as usize), 4).unwrap_err().contains("места приложения (4 МБ)"));
        // The same name replaces: its own size does not count twice.
        files.put("day", "today", &png(3 * MB as usize), 4).unwrap();
        assert_eq!(files.usage(Some("day")), 3 * MB);
        assert!(files.put("day", "text", b"<html>", 4).unwrap_err().contains("это не картинка"));
        // The station: 1 MB a picture, 5 MB for all plugins together.
        station.store(true, Ordering::Relaxed);
        assert!(files.put("cats", "big", &png(2 * MB as usize), 64).unwrap_err().contains("до 1 МБ на картинку"));
        files.put("cats", "one", &png(MB as usize), 64).unwrap();
        files.put("cats", "two", &png(MB as usize), 64).unwrap();
        assert!(
            files.put("cats", "three", &png(MB as usize), 64).unwrap_err().contains("до 5 МБ"),
            "3 + 1 + 1 + 1 > 5"
        );
        assert_eq!(files.view("cats", 64)["limitBytes"], STATION_ALL_MAX);
        // Listed newest first, read for the panel, deleted, forgotten with the plugin.
        assert_eq!(files.list("day")[0]["name"], "today.png");
        assert_eq!(files.read("day", "today.png").unwrap().1, "image/png");
        assert!(files.read("day", "../cats/one.png").is_none());
        assert!(files.delete("cats", "one.png") && !files.delete("cats", "one.png"));
        files.forget("day");
        assert_eq!(files.usage(None), MB, "only «cats» two.png is left");
    }
}
