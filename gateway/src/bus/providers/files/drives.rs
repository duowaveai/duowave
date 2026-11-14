//! Drives and dropped folders offered to «Подключено» (docs/plans/shared-folders.md, «Подключено — внешнее по
//! желанию»). Nothing is attached by itself the first time: a drive plugged in, or a folder dropped on the app's
//! window, becomes an offer — a tile on the desk with «Посмотреть» and «Не надо», and for a drive the station asks too
//! when it is free (the app «Папки», `on_offer`). A drive the household looked at once is known again by its mark (its
//! label and size) and comes back by itself; one not remembered is taken away when it is pulled out. «Не надо» holds
//! until the drive is pulled out.
//!
//! The watcher looks every few seconds: a Mac's `/Volumes`, Linux's mounts under `/media` and `/run/media`, Windows'
//! drive letters that came after the start. Only a computer's gateway watches; the station has no drives.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

use crate::core::events::{EventBus, GatewayEvent};
use crate::utils::time::now_iso;

use super::rules::{clean_name, Folders, How, Rule};

/// How often the drives are looked at.
pub const LOOK_EVERY: Duration = Duration::from_secs(3);

/// A drive mounted on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// Its label and size: the same drive plugged in again has the same mark.
    pub mark: String,
    /// What the system calls it: «KINGSTON», «NIKON D3500».
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OfferKind {
    Drive,
    Dropped,
}

/// Something a person may attach with one press or one «да».
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub id: String,
    pub kind: OfferKind,
    /// The name it would have in «Подключено».
    pub name: String,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub mark: Option<String>,
    pub at: String,
}

/// What is done when a drive is offered.
pub type OnOffer = Arc<dyn Fn(&Offer) + Send + Sync>;

/// The offers and the drives last seen.
pub struct Drives {
    folders: Arc<Folders>,
    events: Option<EventBus>,
    offers: Mutex<Vec<Offer>>,
    /// Drives a person said «не надо» to, until they are pulled out.
    declined: Mutex<HashSet<String>>,
    seen: Mutex<Vec<Drive>>,
    /// Windows: the letters there at the start are the computer's own disks, not drives plugged in.
    first: Mutex<Option<HashSet<PathBuf>>>,
    /// A drive offered: the station may ask about it (the app «Папки»).
    on_offer: OnceLock<OnOffer>,
}

/// An offer's id: stable for a drive (by its mark), new for each dropped folder.
fn drive_id(mark: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(mark.as_bytes());
    format!("d{}", hex::encode(&digest[..5]))
}

impl Drives {
    pub fn new(folders: Arc<Folders>, events: Option<EventBus>) -> Arc<Drives> {
        Arc::new(Drives {
            folders,
            events,
            offers: Mutex::new(Vec::new()),
            declined: Mutex::new(HashSet::new()),
            seen: Mutex::new(Vec::new()),
            first: Mutex::new(None),
            on_offer: OnceLock::new(),
        })
    }

    /// What is done when a drive is offered: the station's question, set by the gateway at its start.
    pub fn set_on_offer(&self, hook: OnOffer) {
        let _ = self.on_offer.set(hook);
    }

    fn told(&self, what: &str) {
        if let Some(events) = &self.events {
            events.emit(GatewayEvent::FoldersChanged { what: what.into() });
        }
    }

    pub fn offers(&self) -> Vec<Offer> {
        self.offers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn offer(&self, id: &str) -> Option<Offer> {
        self.offers().into_iter().find(|offer| offer.id == id)
    }

    /// Looks every few seconds, off the runtime's threads, for as long as the gateway runs.
    pub fn start(self: &Arc<Self>) {
        let drives = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(LOOK_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(this) = drives.upgrade() else { return };
                let now = tokio::task::spawn_blocking(scan).await.unwrap_or_default();
                this.step(now);
            }
        });
    }

    /// One look: the drives now against the drives before.
    pub fn step(&self, now: Vec<Drive>) {
        let now = self.after_start(now);
        let before =
            std::mem::replace(&mut *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()), now.clone());
        let mut changed = false;
        let mut offered = Vec::new();
        for drive in &now {
            if let Some(rule) = self.folders.by_volume(&drive.mark) {
                changed |= self.folders.moved(&drive.mark, &drive.path);
                changed |= !before.iter().any(|seen| seen.mark == drive.mark) && rule.attached();
                continue;
            }
            let declined = self.declined.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(&drive.mark);
            let mut offers = self.offers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if declined || offers.iter().any(|offer| offer.mark.as_deref() == Some(drive.mark.as_str())) {
                continue;
            }
            let offer = Offer {
                id: drive_id(&drive.mark),
                kind: OfferKind::Drive,
                name: format!("Флешка {}", clean_name(&drive.label)),
                path: drive.path.clone(),
                mark: Some(drive.mark.clone()),
                at: now_iso(),
            };
            offers.push(offer.clone());
            offered.push(offer);
            changed = true;
        }
        for gone in before.iter().filter(|seen| !now.iter().any(|drive| drive.mark == seen.mark)) {
            self.offers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|offer| offer.mark.as_deref() != Some(gone.mark.as_str()));
            self.declined.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&gone.mark);
            if let Some(rule) = self.folders.by_volume(&gone.mark).filter(|rule| !rule.remember) {
                let _ = self.folders.detach(&rule.name());
            }
            changed = true;
        }
        if changed {
            self.folders.apply();
            self.told("drives");
        }
        if let Some(hook) = self.on_offer.get() {
            for offer in &offered {
                hook(offer);
            }
        }
    }

    /// On Windows only the letters that came after the start are drives plugged in.
    fn after_start(&self, now: Vec<Drive>) -> Vec<Drive> {
        if !cfg!(windows) {
            return now;
        }
        let mut first = self.first.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let first = first.get_or_insert_with(|| now.iter().map(|drive| drive.path.clone()).collect());
        now.into_iter().filter(|drive| !first.contains(&drive.path)).collect()
    }

    /// A folder dropped on the desk in the app's window: an offer the admin confirms with a press.
    pub fn offer_dropped(&self, path: &Path) -> Result<Offer, String> {
        if !path.is_dir() {
            return Err(crate::tr!("{path} — не папка", path = path.display()));
        }
        let name = path.file_name().map(|name| clean_name(&name.to_string_lossy())).unwrap_or_else(|| "Папка".into());
        let offer = Offer {
            id: format!("f{}", crate::utils::ids::random_hex(5)),
            kind: OfferKind::Dropped,
            name,
            path: path.to_path_buf(),
            mark: None,
            at: now_iso(),
        };
        let mut offers = self.offers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(known) = offers.iter().find(|known| known.path == offer.path) {
            return Ok(known.clone());
        }
        offers.push(offer.clone());
        drop(offers);
        self.told("offer");
        Ok(offer)
    }

    /// «Посмотреть»: the offer attached; a drive is remembered to come back by itself.
    pub fn look(&self, id: &str, by: Option<&str>) -> Result<Rule, String> {
        let offer = self.offer(id).ok_or_else(|| crate::tr!("этого предложения уже нет"))?;
        let how = match offer.kind {
            OfferKind::Drive => How::Drive,
            OfferKind::Dropped => How::Dropped,
        };
        let rule = self.folders.attach(&offer.path, Some(&offer.name), how, offer.mark.clone(), by)?;
        self.offers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|known| known.id != id);
        self.told("offer");
        Ok(rule)
    }

    /// «Не надо»: a drive is not offered again until it is pulled out.
    pub fn skip(&self, id: &str) -> Result<(), String> {
        let offer = self.offer(id).ok_or_else(|| crate::tr!("этого предложения уже нет"))?;
        if let Some(mark) = &offer.mark {
            self.declined.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(mark.clone());
        }
        self.offers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|known| known.id != id);
        self.told("offer");
        Ok(())
    }
}

/// The drives mounted now.
pub fn scan() -> Vec<Drive> {
    let mut drives = system_drives();
    drives.sort_by(|a, b| a.path.cmp(&b.path));
    drives
}

/// A mark of a drive: its label and its size, the same each time it is plugged in.
fn mark(label: &str, size: u64) -> String {
    format!("{label}:{size}")
}

#[cfg(target_os = "macos")]
fn system_drives() -> Vec<Drive> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    // The flags of `statfs` (sys/mount.h).
    const MNT_RDONLY: u32 = 0x0000_0001;
    const MNT_LOCAL: u32 = 0x0000_1000;
    const MNT_DONTBROWSE: u32 = 0x0010_0000;
    let mut drives = Vec::new();
    for entry in std::fs::read_dir("/Volumes").into_iter().flatten().flatten() {
        let label = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        // The system's own disk is a link to «/» here; disk images and backups show as read-only or hidden volumes.
        if label.starts_with('.') || label.starts_with("com.apple") {
            continue;
        }
        if path.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(true) {
            continue;
        }
        let Ok(name) = CString::new(path.as_os_str().as_bytes()) else { continue };
        // SAFETY: a zeroed statfs is a valid buffer for statfs(2) to fill; the path is NUL-terminated.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(name.as_ptr(), &mut stat) } != 0 {
            continue;
        }
        let flags = stat.f_flags;
        if flags & MNT_RDONLY != 0 || flags & MNT_LOCAL == 0 || flags & MNT_DONTBROWSE != 0 {
            continue;
        }
        let size = stat.f_blocks.saturating_mul(stat.f_bsize as u64);
        drives.push(Drive { mark: mark(&label, size), label, path });
    }
    drives
}

#[cfg(target_os = "linux")]
fn system_drives() -> Vec<Drive> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    let mut drives = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(point), Some(options)) = (fields.get(1), fields.get(3)) else { continue };
        // Spaces and the like come escaped as \040.
        let point = unescape_mount(point);
        if !(point.starts_with("/media/") || point.starts_with("/run/media/")) {
            continue;
        }
        if !options.split(',').any(|option| option == "rw") {
            continue;
        }
        let path = PathBuf::from(&point);
        let label = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let Ok(name) = CString::new(path.as_os_str().as_bytes()) else { continue };
        // SAFETY: a zeroed statvfs is a valid buffer for statvfs(3) to fill; the path is NUL-terminated.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(name.as_ptr(), &mut stat) } != 0 {
            continue;
        }
        let size = (stat.f_blocks as u64).saturating_mul(stat.f_frsize as u64);
        drives.push(Drive { mark: mark(&label, size), label, path });
    }
    drives
}

#[cfg(target_os = "linux")]
fn unescape_mount(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut raw = Vec::new();
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 4 <= bytes.len()
            && bytes[index + 1..index + 4].iter().all(u8::is_ascii_digit)
        {
            let code = u8::from_str_radix(&text[index + 1..index + 4], 8).unwrap_or(b'?');
            raw.push(code);
            index += 4;
        } else {
            raw.push(bytes[index]);
            index += 1;
        }
    }
    out.push_str(&String::from_utf8_lossy(&raw));
    out
}

#[cfg(windows)]
fn system_drives() -> Vec<Drive> {
    ('D'..='Z')
        .filter_map(|letter| {
            let path = PathBuf::from(format!("{letter}:\\"));
            path.is_dir().then(|| Drive { mark: format!("{letter}:"), label: format!("{letter}:"), path })
        })
        .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn system_drives() -> Vec<Drive> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::providers::files::places::Places;
    use crate::storage::Db;

    fn made() -> (Arc<Drives>, Arc<Folders>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), false)
            .with_home(Some(dir.path().join("DuoWave")));
        places.prepare();
        let folders = Folders::new(Arc::new(Db::memory()), places, None);
        (Drives::new(folders.clone(), None), folders, dir)
    }

    fn drive(dir: &Path, label: &str) -> Drive {
        let path = dir.join("Volumes").join(label);
        std::fs::create_dir_all(&path).unwrap();
        Drive { mark: mark(label, 64), label: label.into(), path }
    }

    #[test]
    fn a_drive_is_offered_looked_at_and_known_again() {
        let (drives, folders, dir) = made();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let heard = asked.clone();
        drives.set_on_offer(Arc::new(move |offer: &Offer| heard.lock().unwrap().push(offer.name.clone())));
        let usb = drive(dir.path(), "KINGSTON");
        drives.step(vec![usb.clone()]);
        let offers = drives.offers();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].name, "Флешка KINGSTON");
        assert_eq!(*asked.lock().unwrap(), vec!["Флешка KINGSTON".to_string()]);
        drives.step(vec![usb.clone()]);
        assert_eq!(drives.offers().len(), 1, "one offer a drive");
        let rule = drives.look(&offers[0].id, Some("u1")).unwrap();
        assert_eq!(rule.address.as_str(), "files/home/Подключено/Флешка KINGSTON");
        assert!(drives.offers().is_empty());
        // Pulled out: remembered, its link gone; plugged in again at another place: there by itself.
        drives.step(vec![]);
        assert_eq!(folders.attached().len(), 1);
        let again = Drive { path: dir.path().join("Volumes/KINGSTON 1"), ..usb };
        std::fs::create_dir_all(&again.path).unwrap();
        drives.step(vec![again.clone()]);
        assert!(drives.offers().is_empty(), "known again, not offered");
        assert_eq!(folders.attached()[0].path.as_deref(), Some(again.path.as_path()));
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn not_now_holds_until_the_drive_is_pulled_out() {
        let (drives, folders, dir) = made();
        let usb = drive(dir.path(), "CAMERA");
        drives.step(vec![usb.clone()]);
        let id = drives.offers()[0].id.clone();
        drives.skip(&id).unwrap();
        drives.step(vec![usb.clone()]);
        assert!(drives.offers().is_empty());
        drives.step(vec![]);
        drives.step(vec![usb]);
        assert_eq!(drives.offers().len(), 1, "offered again after it was plugged in again");
        assert!(folders.attached().is_empty());
    }

    #[test]
    fn a_drive_not_remembered_goes_when_pulled_out() {
        let (drives, folders, dir) = made();
        let usb = drive(dir.path(), "STICK");
        drives.step(vec![usb.clone()]);
        let rule = drives.look(&drives.offers()[0].id, None).unwrap();
        folders
            .change(&rule.address, &super::super::rules::Change { remember: Some(false), ..Default::default() })
            .unwrap();
        drives.step(vec![]);
        assert!(folders.attached().is_empty());
    }

    #[test]
    fn a_dropped_folder_waits_for_a_press() {
        let (drives, folders, dir) = made();
        let trip = dir.path().join("Отпуск");
        std::fs::create_dir_all(&trip).unwrap();
        let offer = drives.offer_dropped(&trip).unwrap();
        assert_eq!((offer.kind, offer.name.as_str()), (OfferKind::Dropped, "Отпуск"));
        assert_eq!(drives.offer_dropped(&trip).unwrap().id, offer.id, "dropped twice is one offer");
        assert!(folders.attached().is_empty());
        drives.look(&offer.id, None).unwrap();
        assert_eq!(folders.attached()[0].how, Some(How::Dropped));
        assert!(drives.offer_dropped(&dir.path().join("nothing")).is_err());
    }
}
