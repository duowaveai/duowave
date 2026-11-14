//! Folders and files on the bus (`files/…`, docs/plans/bus.md, «Папки»; docs/plans/shared-folders.md). A folder is
//! an address like any other, on the same ladder: see — its list, listen — «a file came», call — read, edit — put,
//! change, remove. The rights are the bus's: this provider has none of its own, it only knows where the bytes live
//! (`places.rs`) and keeps them in bounds (`path.rs`).
//!
//! - `files/app/<id>/…` — an app's own folder: free for it, unseen by others; `tmp/` in it is cleaned by itself.
//! - `files/home/<папка>/…` — the household's, in the home folder (`~/DuoWave` in the app): Фото, Документы, Видео,
//!   Загрузки, Картинки; Музыка and Сказки are the household's own music and tales (CONTENT_DIR), read only.
//! - `files/home/Подключено/<имя>/…` — what a person attached: a drive, a folder dropped on the desk or picked.
//! - `files/person/<id>/…` — a person's: personal, open only to a link made for it.
//! - `files/project/<id>/…` — a project's, in the home folder's «Проекты».
//!
//! Removing a file of the household's, a person's or a project's puts it in the bin for a week.

pub mod drives;
pub mod find;
pub mod path;
pub mod places;
pub mod preview;
pub mod routes;
pub mod rules;
pub mod text;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};

use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, Participant, Provider};
use crate::bus::trail::Authority;
use crate::bus::{Bus, Level};
use crate::core::identity::roles::Role;
use drives::Drives;
use rules::{Folders, Rule};

pub use places::{
    AttachedPlace, Kind, Places, Spot, ATTACHED, BIN_TTL, CONTENT_FOLDERS, HOME_FOLDERS, PROJECTS, TMP_TTL,
};

/// A file read or written through a letter at most: JSON carries it, the app's memory holds it.
pub const COMPUTER_FILE_MAX: u64 = 8 * 1024 * 1024;
pub const STATION_FILE_MAX: u64 = 1024 * 1024;
/// On the station all the files together, apps' and the household's: its disk is small (decision 6).
pub const STATION_ALL_MAX: u64 = 5 * 1024 * 1024;
/// An app's own folder on a computer when its manifest names no other.
pub const APP_FOLDER_MB: u64 = 64;
/// The built-in app that walks the home folder by voice (packages/folders): it asks about a drive plugged in.
pub const FOLDERS_APP: &str = "folders";
/// A folder's list: a page of this many entries, and at most.
pub const PAGE: usize = 200;
pub const PAGE_MOST: usize = 500;
/// How long a link for the station's player to one file of a folder holds.
pub const PLAY_FOR: Duration = Duration::from_secs(6 * 3600);

/// Names in the order people expect: «2» before «10», case aside.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(text: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in text.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((was, chunk)) if *was == digit => chunk.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (left, right) = (chunks(a), chunks(b));
    for ((a_digit, a_chunk), (b_digit, b_chunk)) in left.iter().zip(right.iter()) {
        let order = if *a_digit && *b_digit {
            let (a_trim, b_trim) = (a_chunk.trim_start_matches('0'), b_chunk.trim_start_matches('0'));
            a_trim.len().cmp(&b_trim.len()).then_with(|| a_trim.cmp(b_trim))
        } else {
            a_chunk.cmp(b_chunk)
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len()).then_with(|| a.cmp(b))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    name: String,
    address: String,
    folder: bool,
    /// What it is, for a list read aloud or drawn: `folder`, `image`, `text`, `pdf`, `doc`, `audio`, `video`,
    /// `other`.
    kind: &'static str,
    size: u64,
    modified: Option<String>,
}

/// What a file is, by its name.
pub fn kind_of(name: &str, folder: bool) -> &'static str {
    if folder {
        return "folder";
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif" | "bmp" | "tif" | "tiff" => "image",
        "txt" | "md" | "markdown" | "csv" | "log" | "json" | "rtf" => "text",
        "pdf" => "pdf",
        "doc" | "docx" | "odt" | "pages" | "xls" | "xlsx" | "ppt" | "pptx" => "doc",
        "mp3" | "ogg" | "opus" | "m4a" | "aac" | "flac" | "wav" => "audio",
        "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm" => "video",
        _ => "other",
    }
}

fn item(address: &Address, path: &Path) -> Option<Item> {
    let meta = std::fs::metadata(path).ok()?;
    let name = address.segments().last().map(str::to_string).unwrap_or_default();
    Some(Item {
        kind: kind_of(&name, meta.is_dir()),
        name,
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
    /// The household's word on its folders and what is attached (rules.rs).
    folders: OnceLock<Arc<Folders>>,
    /// Drives and dropped folders offered (drives.rs): a computer's gateway only.
    drives: OnceLock<Arc<Drives>>,
    /// Whether the station says our answers in our own voice (not Yandex's): then it may name the files.
    own_voice: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Whether the model of «Мозг» is in the cloud: then what it reads leaves the house.
    cloud_model: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// The folders' walks for «найди».
    finder: find::Finder,
    /// Links for the station's player to one file each: a token, the file, until when.
    plays: Mutex<HashMap<String, (PathBuf, Instant)>>,
}

impl Files {
    pub fn new(places: Places, quota: Arc<dyn Fn(&str) -> u64 + Send + Sync>) -> Files {
        Files {
            places,
            bus: OnceLock::new(),
            quota,
            folders: OnceLock::new(),
            drives: OnceLock::new(),
            own_voice: OnceLock::new(),
            cloud_model: OnceLock::new(),
            finder: find::Finder::default(),
            plays: Mutex::new(HashMap::new()),
        }
    }

    /// The file a player's link names, while it holds (`/media/folder/<token>`).
    pub fn played(&self, token: &str) -> Option<PathBuf> {
        let mut plays = self.plays.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        plays.retain(|_, (_, until)| *until > Instant::now());
        plays.get(token).map(|(path, _)| path.clone())
    }

    /// A link for the station's player to one file: an unguessable token, for six hours.
    fn mint(&self, path: PathBuf) -> String {
        let token = crate::utils::ids::random_hex(24);
        let mut plays = self.plays.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        plays.retain(|_, (_, until)| *until > Instant::now());
        plays.insert(token.clone(), (path, Instant::now() + PLAY_FOR));
        token
    }

    /// Where the pictures made small are kept.
    pub fn previews(&self) -> PathBuf {
        self.places
            .files
            .parent()
            .map(|data| data.join("folder-previews"))
            .unwrap_or_else(|| PathBuf::from("folder-previews"))
    }

    /// A picture of a folder made small, for the panel (its own route) and the apps (`preview`).
    pub fn preview_of(&self, path: &Path, side: u32) -> Result<Vec<u8>, BusError> {
        preview::preview(path, side, &self.previews()).map_err(BusError::Failed)
    }

    /// What leaves the house is the gateway's to know: the station's voice and the model's place.
    pub fn set_leaving(
        &self,
        own_voice: Arc<dyn Fn() -> bool + Send + Sync>,
        cloud_model: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        let _ = self.own_voice.set(own_voice);
        let _ = self.cloud_model.set(cloud_model);
    }

    /// What leaves the house now, for the panel's amber: (the voice is Yandex's, the model is in the cloud).
    pub fn leaving(&self) -> (bool, bool) {
        let yandex_voice = !self.own_voice.get().is_some_and(|own| own());
        let cloud_model = self.cloud_model.get().is_some_and(|cloud| cloud());
        (yandex_voice, cloud_model)
    }

    /// The household's word on a folder: its row, or the defaults.
    pub fn rule(&self, folder: &Address) -> Rule {
        self.folders().map(|folders| folders.rule(folder)).unwrap_or_else(|| Rule::defaults(folder.clone()))
    }

    /// Whether the voice may name this folder's files: our own voice says them, or the household ticked it though
    /// Yandex's voice says them.
    pub fn may_speak(&self, folder: &Address) -> bool {
        self.rule(folder).speak || self.own_voice.get().is_some_and(|own| own())
    }

    /// Whether the model may read this folder: our own model reads it at home, or the household ticked it though the
    /// model is in the cloud.
    pub fn may_model(&self, folder: &Address) -> bool {
        self.rule(folder).model || self.cloud_model.get().is_none_or(|cloud| !cloud())
    }

    /// The folders' table, its attached folders put in their places.
    pub fn set_folders(&self, folders: Arc<Folders>) {
        folders.apply();
        let _ = self.folders.set(folders);
    }

    pub fn folders(&self) -> Option<Arc<Folders>> {
        self.folders.get().cloned()
    }

    /// The drives' watcher, where something may be attached.
    pub fn set_drives(&self, drives: Arc<Drives>) {
        let _ = self.drives.set(drives);
    }

    pub fn drives(&self) -> Option<Arc<Drives>> {
        self.drives.get().cloned()
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
                return Err(BusError::Failed(crate::tr!(
                    "на колонке места под файлы нет: занято {used} КБ из {max} КБ",
                    used = used / 1024,
                    max = STATION_ALL_MAX / 1024
                )));
            }
        }
        if let Some(app) = spot.owner.as_ref().filter(|owner| owner.kind() == address::APP).and_then(Address::name) {
            let quota = (self.quota)(app) * 1024 * 1024;
            let used = Places::size_of(&spot.root);
            if used + bytes > quota {
                return Err(BusError::Failed(crate::tr!(
                    "своя папка полна: занято {used} КБ из {max} КБ",
                    used = used / 1024,
                    max = quota / 1024
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

    /// The folder's root made where the gateway owns it (an app's, a person's, a project's, the household's); what a
    /// person attached is never made: when it is not there, it is not there.
    fn made(&self, spot: &Spot) -> Result<(), BusError> {
        if spot.kind == Kind::Attached || spot.kind == Kind::Content || spot.kind == Kind::AttachedList {
            return Ok(());
        }
        std::fs::create_dir_all(&spot.root).map_err(|error| BusError::Failed(error.to_string()))
    }

    /// Why a path was not given, in words: an attached folder that is gone says so («Флешки сейчас нет»), anything
    /// else not found reads as no such address.
    fn refused(&self, spot: &Spot, letter: &Letter, why: path::Refused) -> BusError {
        match why {
            path::Refused::Missing if spot.kind == Kind::Attached => {
                BusError::NotFound(crate::tr!("папки «{name}» сейчас нет", name = self.title(&spot.folder)))
            }
            _ => BusError::NotFound(letter.to.to_string()),
        }
    }

    /// The folder's title for people.
    fn title(&self, folder: &Address) -> String {
        match folder.segments().collect::<Vec<_>>().as_slice() {
            ["app", id] => self.bus().map(|bus| bus.title_of(&Address::app(id))).unwrap_or_else(|| id.to_string()),
            ["home", name] => name.to_string(),
            ["home", _, item] => item.to_string(),
            ["person", id] => crate::tr!("личное {id}", id = id),
            ["project", id] => crate::tr!("проект {id}", id = id),
            _ => folder.to_string(),
        }
    }

    /// Puts a file or a folder in the spot's bin, or removes it at once where there is none.
    fn discard(&self, spot: &Spot, entry: &Path) -> Result<bool, BusError> {
        let failed = |error: std::io::Error| BusError::Failed(error.to_string());
        match &spot.bin {
            Some(bin) => {
                let into = bin.join(crate::utils::ids::random_hex(6));
                std::fs::create_dir_all(&into).map_err(failed)?;
                let name = entry.file_name().map(|name| name.to_os_string()).unwrap_or_default();
                path::move_any(entry, &into.join(name)).map_err(failed)?;
                Ok(true)
            }
            None => path::remove_any(entry).map(|()| false).map_err(failed),
        }
    }

    /// What is attached, as the list of «Подключено».
    fn attached_list(&self) -> Value {
        let items: Vec<Value> = self
            .places
            .attached()
            .into_iter()
            .filter_map(|attached| {
                let address = Address::parse(&format!("files/home/{ATTACHED}/{}", attached.name)).ok()?;
                let here = attached.path.is_dir();
                Some(json!({ "name": attached.name, "address": address, "folder": true, "size": 0, "here": here }))
            })
            .collect();
        json!({ "items": items })
    }

    /// «Посмотреть» or «не надо» to a drive offered, said to the app «Папки»: a person's own word — the chain a person
    /// started (their voice, their press), never an app on its own or a guest. A folder of the disk is attached by the
    /// admin's press only (routes.rs).
    fn answer_offer(&self, letter: &Letter) -> Result<Value, BusError> {
        let person = letter.trail.authority() == Authority::Person
            && letter.trail.root.principal().is_some_and(|principal| principal.role != Some(Role::Guest));
        if !person {
            return Err(BusError::Denied(crate::tr!("открыть флешку можно только по слову человека")));
        }
        let drives = self.drives().ok_or_else(|| BusError::Failed(crate::tr!("здесь флешки не подключают")))?;
        let id = match letter.input.get("offer").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => drives
                .offers()
                .into_iter()
                .rev()
                .find(|offer| offer.kind == drives::OfferKind::Drive)
                .map(|offer| offer.id)
                .ok_or_else(|| BusError::NotFound(crate::tr!("новой флешки нет")))?,
        };
        let offer = drives.offer(&id).ok_or_else(|| BusError::NotFound(crate::tr!("этого предложения уже нет")))?;
        if offer.kind != drives::OfferKind::Drive {
            return Err(BusError::Denied(crate::tr!("папку с диска подключают нажатием в пульте")));
        }
        let by = letter.trail.root.principal().map(|principal| principal.id.clone());
        if letter.handle == "skip" {
            drives.skip(&id).map_err(BusError::Failed)?;
            return Ok(json!({ "skipped": id }));
        }
        let rule = drives.look(&id, by.as_deref()).map_err(BusError::Failed)?;
        Ok(json!({ "address": rule.address, "name": rule.name() }))
    }

    /// The household's word stands before any letter: outside the home a folder opens only with «и вне дома»; a
    /// chain through a model in the cloud reads nothing of a folder without «Модель может читать» — neither names nor
    /// files (the owner's decision 3, 13.11).
    fn word(&self, spot: &Spot, letter: &Letter) -> Result<(), BusError> {
        if matches!(spot.kind, Kind::App | Kind::Person) {
            return Ok(());
        }
        let rule = self.rule(&spot.folder);
        if letter.trail.remote && !rule.remote {
            return Err(BusError::NotFound(letter.to.to_string()));
        }
        let reads =
            matches!(letter.handle.as_str(), "list" | "stat" | "read" | "copy" | "find" | "text" | "preview" | "media");
        if reads && letter.trail.through_cloud() && !rule.model {
            return Err(BusError::Denied(crate::tr!(
                "модель «Мозга» в облаке, а папка «{name}» ей закрыта: галочка «Модель может читать» в разделе «Папки»",
                name = self.title(&spot.folder)
            )));
        }
        Ok(())
    }

    fn act(&self, letter: &Letter) -> Result<Value, BusError> {
        let spot = self.places.spot(&letter.to).ok_or_else(|| BusError::NotFound(letter.to.to_string()))?;
        self.word(&spot, letter)?;
        // A change in a folder: its walk for «найди» is made again.
        if matches!(letter.handle.as_str(), "write" | "remove" | "move" | "mkdir" | "copy") {
            self.finder.forget(&spot.root);
            if let Some(target) = letter.input.get("to").and_then(Value::as_str).and_then(|to| Address::parse(to).ok())
            {
                if let Some(target) = self.places.spot(&target) {
                    self.finder.forget(&target.root);
                }
            }
        }
        let input = &letter.input;
        let failed = |error: std::io::Error| BusError::Failed(error.to_string());
        let writes = matches!(letter.handle.as_str(), "write" | "remove" | "move" | "mkdir");
        // A copy puts into its destination only: the source may be read only (a drive, the music).
        if writes && spot.read_only {
            return Err(BusError::Denied(crate::tr!("«{name}» только для чтения", name = self.title(&spot.folder))));
        }
        if spot.kind == Kind::AttachedList {
            return match letter.handle.as_str() {
                "list" => Ok(self.attached_list()),
                "stat" => Ok(json!({ "name": ATTACHED, "address": spot.folder, "folder": true, "size": 0 })),
                "offers" => Ok(json!({ "offers": self.drives().map(|drives| drives.offers()).unwrap_or_default() })),
                "look" | "skip" => self.answer_offer(letter),
                other => Err(BusError::NotFound(format!("{}: {other}", letter.to))),
            };
        }
        // A root the gateway owns is made when something is put there; an app's own folder whenever it is used.
        if writes || spot.kind == Kind::App {
            self.made(&spot)?;
        }
        let refused = |why| self.refused(&spot, letter, why);
        match letter.handle.as_str() {
            "list" => {
                let dir = path::existing(&spot.root, &spot.rest).map_err(refused)?;
                let base = path::root(&spot.root).map_err(refused)?;
                let entries = std::fs::read_dir(&dir).map_err(|_| BusError::NotFound(letter.to.to_string()))?;
                let mut items: Vec<Item> = entries
                    .flatten()
                    .filter(|entry| path::shown(&base, &entry.path()))
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        item(&letter.to.join(&name).ok()?, &entry.path())
                    })
                    .collect();
                // Folders first, then names as people count: «2» before «10».
                items.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| natural(&a.name, &b.name)));
                let total = items.len();
                let from = input.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
                let limit = input.get("limit").and_then(Value::as_u64).map(|limit| limit as usize).unwrap_or(PAGE);
                let page: Vec<Item> = items.into_iter().skip(from).take(limit.clamp(1, PAGE_MOST)).collect();
                let next = (from + page.len() < total).then_some(from + page.len());
                Ok(json!({ "items": page, "total": total, "next": next }))
            }
            "find" => {
                let query = input.get("query").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if query.is_empty() {
                    return Err(BusError::Failed(crate::tr!("find: {query: что искать}")));
                }
                let dir = path::existing(&spot.root, &spot.rest).map_err(refused)?;
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
                let (hits, partial) = self.finder.find(&dir, &query, limit).map_err(refused)?;
                let hits: Vec<Value> = hits
                    .into_iter()
                    .filter_map(|hit| {
                        let address = letter.to.join(&hit.path).ok()?;
                        let kind = kind_of(&hit.name, hit.folder);
                        Some(json!({ "address": address, "name": hit.name, "folder": hit.folder, "kind": kind, "score": hit.score }))
                    })
                    .collect();
                Ok(json!({ "items": hits, "partial": partial }))
            }
            "text" => {
                let (_, found) = path::open(&spot.root, &spot.rest).map_err(refused)?;
                let text = text::text_of(&found).map_err(BusError::Failed)?;
                let part = input.get("part").and_then(Value::as_u64).unwrap_or(0) as usize;
                let (piece, parts) = text::part(&text, part);
                Ok(json!({ "text": piece, "part": part, "parts": parts }))
            }
            "preview" => {
                let (_, found) = path::open(&spot.root, &spot.rest).map_err(refused)?;
                let side = input.get("size").and_then(Value::as_u64).unwrap_or(preview::SIDE as u64) as u32;
                let bytes = self.preview_of(&found, side)?;
                Ok(json!({ "base64": B64.encode(&bytes), "type": "image/jpeg", "size": bytes.len() }))
            }
            "play" => {
                let (_, found) = path::open(&spot.root, &spot.rest).map_err(refused)?;
                let name = found.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                if kind_of(&name, false) != "audio" {
                    return Err(BusError::Failed(crate::tr!("это не звук: колонка играет mp3, ogg, m4a, flac, wav")));
                }
                let title = name.rsplit_once('.').map(|(stem, _)| stem.to_string()).unwrap_or_else(|| name.clone());
                let token = self.mint(found);
                Ok(json!({ "id": format!("folder-{}", &token[..12]), "title": title, "file": token }))
            }
            // A link for the computer's screen (src/screen): a photo, a video or a sound in a window of its own.
            "media" => {
                let (_, found) = path::open(&spot.root, &spot.rest).map_err(refused)?;
                let name = found.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                let kind = kind_of(&name, false);
                if !matches!(kind, "image" | "video" | "audio") {
                    return Err(BusError::Failed(crate::tr!("на экране показываются фото, видео и звук")));
                }
                let title = name.rsplit_once('.').map(|(stem, _)| stem.to_string()).unwrap_or_else(|| name.clone());
                let token = self.mint(found);
                Ok(json!({ "kind": kind, "title": title, "file": token }))
            }
            "stat" => {
                let found = path::existing(&spot.root, &spot.rest).map_err(refused)?;
                let mut stat = item(&letter.to, &found)
                    .map(|item| json!(item))
                    .ok_or_else(|| BusError::NotFound(letter.to.to_string()))?;
                // What leaves the house, as the gateway counts it: the app says names aloud only with `speak`.
                stat["speak"] = json!(self.may_speak(&spot.folder));
                stat["model"] = json!(self.may_model(&spot.folder));
                Ok(stat)
            }
            "read" => {
                let (mut file, _) = path::open(&spot.root, &spot.rest).map_err(refused)?;
                let meta = file.metadata().map_err(failed)?;
                if meta.is_dir() {
                    return Err(BusError::Failed(crate::tr!("это папка: её список — list")));
                }
                if meta.len() > self.max_file() {
                    return Err(BusError::Failed(crate::tr!(
                        "файл больше {max} МБ: такой читают по частям или ссылкой",
                        max = self.max_file() / 1024 / 1024
                    )));
                }
                let mut bytes = Vec::with_capacity(meta.len() as usize);
                file.read_to_end(&mut bytes).map_err(failed)?;
                let as_text = input.get("as").and_then(Value::as_str) == Some("text");
                Ok(match (as_text, String::from_utf8(bytes.clone())) {
                    (true, Ok(text)) => json!({ "text": text, "size": bytes.len() }),
                    (true, Err(_)) => return Err(BusError::Failed(crate::tr!("файл не текст: читайте без as: text"))),
                    (false, _) => json!({ "base64": B64.encode(&bytes), "size": bytes.len() }),
                })
            }
            "write" => {
                if spot.rest.is_empty() {
                    return Err(BusError::Failed(crate::tr!("write: нужно имя файла в папке")));
                }
                let bytes =
                    match (input.get("text").and_then(Value::as_str), input.get("base64").and_then(Value::as_str)) {
                        (Some(text), _) => text.as_bytes().to_vec(),
                        (None, Some(data)) => B64
                            .decode(data.split_once(',').map(|(_, data)| data).unwrap_or(data))
                            .map_err(|_| BusError::Failed(crate::tr!("base64 не читается")))?,
                        (None, None) => return Err(BusError::Failed(crate::tr!("write: {text} или {base64}"))),
                    };
                if bytes.len() as u64 > self.max_file() {
                    return Err(BusError::Failed(crate::tr!(
                        "файл больше {max} МБ",
                        max = self.max_file() / 1024 / 1024
                    )));
                }
                let target = path::writable(&spot.root, &spot.rest).map_err(refused)?;
                let before = target.symlink_metadata().ok().filter(|meta| meta.is_file()).map(|meta| meta.len());
                // A file of the household's replaced: its old self waits in the bin for a week, as a removed one.
                if before.is_some() {
                    if let Some(bin) = &spot.bin {
                        let into = bin.join(crate::utils::ids::random_hex(6));
                        let name = target.file_name().map(|name| name.to_os_string()).unwrap_or_default();
                        std::fs::create_dir_all(&into)
                            .and_then(|()| path::copy_any(&target, &into.join(name)))
                            .map_err(failed)?;
                    }
                }
                self.room(&spot, (bytes.len() as u64).saturating_sub(before.unwrap_or(0)))?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(failed)?;
                }
                // A temporary file renamed into place: a half-written file is never seen.
                let temp = target.with_file_name(format!(".part-{}", crate::utils::ids::random_hex(4)));
                std::fs::write(&temp, &bytes).map_err(failed)?;
                std::fs::rename(&temp, &target).map_err(|error| {
                    let _ = std::fs::remove_file(&temp);
                    failed(error)
                })?;
                self.tell(
                    letter,
                    &spot.folder,
                    if before.is_none() { "file.added" } else { "file.changed" },
                    &letter.to,
                );
                Ok(json!({ "address": letter.to.to_string(), "size": bytes.len() }))
            }
            "mkdir" => {
                if spot.rest.is_empty() {
                    return Ok(json!({ "address": letter.to.to_string() }));
                }
                let target = path::writable(&spot.root, &spot.rest).map_err(refused)?;
                std::fs::create_dir_all(&target).map_err(failed)?;
                Ok(json!({ "address": letter.to.to_string() }))
            }
            "remove" => {
                if spot.rest.is_empty() {
                    return Err(BusError::Denied(crate::tr!("саму папку не удалить")));
                }
                // The entry itself — a link is taken away as a link, never followed to what it points at.
                let entry = path::entry(&spot.root, &spot.rest).map_err(refused)?;
                let binned = self.discard(&spot, &entry)?;
                self.tell(letter, &spot.folder, "file.removed", &letter.to);
                Ok(json!({ "removed": letter.to.to_string(), "bin": binned }))
            }
            "move" => {
                let to = input
                    .get("to")
                    .and_then(Value::as_str)
                    .and_then(|to| Address::parse(to).ok())
                    .ok_or_else(|| BusError::Failed(crate::tr!("move: {to: адрес, куда}")))?;
                let target = self.places.spot(&to).ok_or_else(|| BusError::NotFound(to.to_string()))?;
                if target.read_only || target.rest.is_empty() || target.kind == Kind::AttachedList {
                    return Err(BusError::Denied(crate::tr!(
                        "в «{name}» не положить",
                        name = self.title(&target.folder)
                    )));
                }
                if spot.rest.is_empty() {
                    return Err(BusError::Denied(crate::tr!("саму папку не перенести")));
                }
                let from = path::entry(&spot.root, &spot.rest).map_err(refused)?;
                self.made(&target)?;
                let destination =
                    path::writable(&target.root, &target.rest).map_err(|why| self.refused(&target, letter, why))?;
                if destination.symlink_metadata().is_ok() {
                    return Err(BusError::Failed(crate::tr!("в «{to}» уже есть такое", to = to)));
                }
                if target.root != spot.root {
                    let size = Places::size_of(&from).max(from.symlink_metadata().map(|meta| meta.len()).unwrap_or(0));
                    self.room(&target, size)?;
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(failed)?;
                }
                path::move_any(&from, &destination).map_err(failed)?;
                self.tell(letter, &spot.folder, "file.removed", &letter.to);
                self.tell(letter, &target.folder, "file.added", &to);
                Ok(json!({ "address": to.to_string() }))
            }
            "copy" => {
                let to = input
                    .get("to")
                    .and_then(Value::as_str)
                    .and_then(|to| Address::parse(to).ok())
                    .ok_or_else(|| BusError::Failed(crate::tr!("copy: {to: адрес, куда}")))?;
                let target = self.places.spot(&to).ok_or_else(|| BusError::NotFound(to.to_string()))?;
                if target.read_only || target.rest.is_empty() || target.kind == Kind::AttachedList {
                    return Err(BusError::Denied(crate::tr!(
                        "в «{name}» не положить",
                        name = self.title(&target.folder)
                    )));
                }
                let from = path::existing(&spot.root, &spot.rest).map_err(refused)?;
                self.made(&target)?;
                let wanted =
                    path::writable(&target.root, &target.rest).map_err(|why| self.refused(&target, letter, why))?;
                let destination = path::free(&wanted);
                let size = Places::size_of(&from).max(std::fs::metadata(&from).map(|meta| meta.len()).unwrap_or(0));
                if size > self.max_file() && !from.is_dir() {
                    return Err(BusError::Failed(crate::tr!(
                        "файл больше {max} МБ",
                        max = self.max_file() / 1024 / 1024
                    )));
                }
                self.room(&target, size)?;
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(failed)?;
                }
                path::copy_any(&from, &destination).map_err(failed)?;
                let mut parts = target.rest.clone();
                if let (Some(last), Some(name)) = (parts.last_mut(), destination.file_name()) {
                    *last = name.to_string_lossy().to_string();
                }
                let copied = target.folder.join(&parts.join("/")).map_err(BusError::Failed)?;
                self.tell(letter, &target.folder, "file.added", &copied);
                Ok(json!({ "address": copied.to_string() }))
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
        let mut add = |path: &str| {
            if let Ok(address) = Address::parse(&format!("files/{path}")) {
                if let Some(participant) = self.participant(&address) {
                    folders.push(participant);
                }
            }
        };
        for name in HOME_FOLDERS {
            add(&format!("home/{name}"));
        }
        // A folder a person made in the home folder in Finder («Рецепты») is the household's too.
        for entry in std::fs::read_dir(&self.places.home).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let reserved = name == ATTACHED
                || name == PROJECTS
                || HOME_FOLDERS.contains(&name.as_str())
                || CONTENT_FOLDERS.iter().any(|(title, _)| *title == name);
            if !reserved && !path::hidden(&name) && entry.path().is_dir() {
                add(&format!("home/{name}"));
            }
        }
        for (name, _) in CONTENT_FOLDERS {
            add(&format!("home/{name}"));
        }
        add(&format!("home/{ATTACHED}"));
        for attached in self.places.attached() {
            add(&format!("home/{ATTACHED}/{}", attached.name));
        }
        for entry in std::fs::read_dir(self.places.files.join("person")).into_iter().flatten().flatten() {
            add(&format!("person/{}", entry.file_name().to_string_lossy()));
        }
        for entry in std::fs::read_dir(self.places.projects()).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !path::hidden(&name) && entry.path().is_dir() {
                add(&format!("project/{name}"));
            }
        }
        folders
    }

    fn participant(&self, address: &Address) -> Option<Participant> {
        let spot = self.places.spot(address)?;
        // A name the questions put in quotes: «менять файлы в папке «Документы»».
        let title = self.title(&spot.folder);
        Some(Participant { address: spot.folder.clone(), title, private: spot.private, owner: spot.owner.clone() })
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        let Some(spot) = self.places.spot(address) else { return Vec::new() };
        // A folder of the home folder is open to the roles the household ticked (an app's and a person's are theirs).
        let roles = (!matches!(spot.kind, Kind::App | Kind::Person)).then(|| self.rule(&spot.folder).roles);
        handles_of(&spot).into_iter().map(|handle| handle.for_roles(roles.clone())).collect()
    }

    /// An app owns its folder; a person their own folder.
    fn owns(&self, from: &Address, to: &Address) -> bool {
        self.places.spot(to).and_then(|spot| spot.owner).is_some_and(|owner| &owner == from)
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        // A move and a copy put into another folder too: the bus checks that destination for the same caller and chain.
        if letter.handle == "move" || letter.handle == "copy" {
            let to = letter.input.get("to").and_then(Value::as_str).and_then(|to| Address::parse(to).ok());
            if let (Some(to), Some(bus)) = (to, self.bus()) {
                match bus.decide(&letter.trail, &letter.from, &to, "write") {
                    Ok(decision) if decision.verdict == crate::bus::policy::Verdict::Allow => {}
                    Ok(_) | Err(_) => {
                        let to = to.to_string();
                        return Box::pin(async move {
                            Err(BusError::Denied(crate::tr!("положить в «{to}» нельзя без связи «правит»", to = to)))
                        });
                    }
                }
            }
        }
        let result = self.act(&letter);
        Box::pin(async move { result })
    }
}

/// What a folder can do, on the ladder.
fn handles_of(spot: &Spot) -> Vec<Handle> {
    let mut handles = vec![
        Handle::new("list", Level::See).titled(&crate::tr!("список")),
        Handle::new("stat", Level::See).titled(&crate::tr!("размер и время")),
    ];
    if spot.kind == Kind::AttachedList {
        handles.push(Handle::new("offers", Level::See).titled(&crate::tr!("что можно подключить")));
        handles.push(Handle::new("look", Level::Call).titled(&crate::tr!("открыть флешку по слову человека")));
        handles.push(Handle::new("skip", Level::Call).titled(&crate::tr!("не открывать флешку")));
        return handles;
    }
    handles.push(Handle::new("find", Level::See).titled(&crate::tr!("искать по именам")));
    handles.push(Handle::new("read", Level::Call).titled(&crate::tr!("читать")));
    handles.push(Handle::new("text", Level::Call).titled(&crate::tr!("читать текст документа")));
    handles.push(Handle::new("preview", Level::Call).titled(&crate::tr!("показать картинку")));
    handles.push(Handle::new("play", Level::Call).titled(&crate::tr!("сыграть на колонке")));
    handles.push(Handle::new("media", Level::Call).titled(&crate::tr!("показать на экране компьютера")));
    handles.push(Handle::new("copy", Level::Call).titled(&crate::tr!("копировать в папку дома")));
    if !spot.read_only {
        handles.push(Handle::new("write", Level::Edit).titled(&crate::tr!("класть и менять")));
        handles.push(Handle::new("mkdir", Level::Edit).titled(&crate::tr!("заводить папки")));
        handles.push(Handle::new("move", Level::Edit).titled(&crate::tr!("переносить")));
        handles.push(Handle::new("remove", Level::Edit).titled(&crate::tr!("удалять")));
    }
    for news in ["file.added", "file.changed", "file.removed"] {
        handles.push(Handle::new(news, Level::Listen).titled(&crate::tr!("новость папки")));
    }
    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(station: bool) -> (Files, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), station);
        std::fs::create_dir_all(dir.path().join("content/music")).unwrap();
        places.prepare();
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
        assert_eq!(list["items"].as_array().unwrap().len(), 1, "no half-written file is seen: {list}");
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
        assert_eq!(Places::size_of(&files.places.home_bin()), 1, "it waits in the home folder's bin");
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

    #[tokio::test]
    async fn a_visible_home_folder_is_where_the_households_files_are() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("DuoWave");
        let places =
            Places::new(&dir.path().join("data"), &dir.path().join("content"), false).with_home(Some(home.clone()));
        places.prepare();
        let files = Files::new(places, Arc::new(|_| 64));
        // A file a person put in Finder is the bus's at once.
        std::fs::write(home.join("Фото/лето.jpg"), "jpeg").unwrap();
        let list = run(&files, "files/home/Фото", "list", json!({})).await.unwrap();
        assert_eq!(list["items"][0]["address"], "files/home/Фото/лето.jpg");
        run(&files, "files/project/сайт/index.html", "write", json!({"text": "<p>"})).await.unwrap();
        assert!(home.join("Проекты/сайт/index.html").is_file());
        #[cfg(unix)]
        {
            // A link out of «Фото» — into «Документы» or to the disk — is no file for the bus.
            std::fs::write(home.join("Документы/паспорт.txt"), "secret").unwrap();
            std::os::unix::fs::symlink(home.join("Документы"), home.join("Фото/туда")).unwrap();
            assert!(matches!(
                run(&files, "files/home/Фото/туда/паспорт.txt", "read", json!({})).await,
                Err(BusError::NotFound(_))
            ));
            let list = run(&files, "files/home/Фото", "list", json!({})).await.unwrap();
            assert_eq!(list["items"].as_array().unwrap().len(), 1, "the link is not listed: {list}");
            // Removing a link takes the link away, never what it points at.
            let removed = run(&files, "files/home/Фото/туда", "remove", json!({})).await;
            assert!(removed.is_err(), "a link out of the folder is not there to remove: {removed:?}");
            assert!(home.join("Документы/паспорт.txt").is_file());
        }
        assert!(matches!(
            run(&files, "files/home/Фото/.hidden", "write", json!({"text": "x"})).await,
            Err(BusError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn what_is_attached_is_listed_and_read_only_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let usb = dir.path().join("usb");
        std::fs::create_dir_all(usb.join("DCIM")).unwrap();
        std::fs::write(usb.join("DCIM/1.jpg"), "jpeg").unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), false);
        places.prepare();
        places.set_attached(vec![AttachedPlace { name: "Флешка".into(), path: usb.clone(), write: false }]);
        let files = Files::new(places, Arc::new(|_| 64));
        let list = run(&files, "files/home/Подключено", "list", json!({})).await.unwrap();
        assert_eq!(list["items"][0]["name"], "Флешка");
        assert_eq!(list["items"][0]["here"], true);
        let read = run(&files, "files/home/Подключено/Флешка/DCIM/1.jpg", "read", json!({"as": "text"})).await.unwrap();
        assert_eq!(read["text"], "jpeg");
        assert!(matches!(
            run(&files, "files/home/Подключено/Флешка/new.txt", "write", json!({"text": "x"})).await,
            Err(BusError::Denied(_))
        ));
        std::fs::remove_dir_all(&usb).unwrap();
        let gone = run(&files, "files/home/Подключено/Флешка", "list", json!({})).await;
        assert!(matches!(gone, Err(BusError::NotFound(ref why)) if why.contains("сейчас нет")), "{gone:?}");
    }

    #[tokio::test]
    async fn a_folder_lists_by_pages_finds_reads_shows_and_plays() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(&dir.path().join("data"), &dir.path().join("content"), false);
        places.prepare();
        let files = Files::new(places.clone(), Arc::new(|_| 64));
        let docs = places.home.join("Документы");
        for n in [10, 2, 1] {
            std::fs::write(docs.join(format!("{n}.txt")), "x").unwrap();
        }
        std::fs::create_dir_all(docs.join("Рецепты")).unwrap();
        std::fs::write(docs.join("Рецепты/Борщ.txt"), "Свёкла\n\nКапуста").unwrap();
        let names = |list: &Value| -> Vec<String> {
            list["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_string()).collect()
        };
        let first = run(&files, "files/home/Документы", "list", json!({"limit": 2})).await.unwrap();
        assert_eq!(names(&first), ["Рецепты", "1.txt"], "folders first, then as people count");
        assert_eq!((&first["total"], &first["next"]), (&json!(4), &json!(2)));
        assert_eq!(first["items"][0]["kind"], "folder");
        let rest = run(&files, "files/home/Документы", "list", json!({"from": 2})).await.unwrap();
        assert_eq!(names(&rest), ["2.txt", "10.txt"]);
        assert!(rest["next"].is_null());
        let found = run(&files, "files/home/Документы", "find", json!({"query": "борщ"})).await.unwrap();
        assert_eq!(found["items"][0]["address"], "files/home/Документы/Рецепты/Борщ.txt");
        let text = run(&files, "files/home/Документы/Рецепты/Борщ.txt", "text", json!({})).await.unwrap();
        assert_eq!((&text["text"], &text["parts"]), (&json!("Свёкла\n\nКапуста"), &json!(1)));
        // A file written through the bus is found at once: the walk is made again.
        run(&files, "files/home/Документы/Борщ зелёный.txt", "write", json!({"text": "щавель"})).await.unwrap();
        let found = run(&files, "files/home/Документы", "find", json!({"query": "борщ"})).await.unwrap();
        assert_eq!(found["items"].as_array().unwrap().len(), 2, "{found}");
        // A picture made small.
        image::RgbImage::from_pixel(800, 600, image::Rgb([10, 20, 30])).save(places.home.join("Фото/a.png")).unwrap();
        let shown = run(&files, "files/home/Фото/a.png", "preview", json!({"size": 200})).await.unwrap();
        let bytes = B64.decode(shown["base64"].as_str().unwrap()).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 200);
        // A song played on the station by a link for one file.
        std::fs::create_dir_all(places.content.join("music")).unwrap();
        std::fs::write(places.content.join("music/песня.mp3"), "mp3").unwrap();
        let played = run(&files, "files/home/Музыка/песня.mp3", "play", json!({})).await.unwrap();
        let token = played["file"].as_str().unwrap();
        assert!(files.played(token).unwrap().ends_with("песня.mp3"));
        assert!(files.played("0".repeat(48).as_str()).is_none());
        let kind = crate::plugins::media::kind_for(crate::skills::capabilities::CapabilityName::Music);
        let item = crate::plugins::media::media_item("folders", kind, &played, Some("http://gw:80/media")).unwrap();
        assert_eq!(item.url, format!("http://gw:80/media/folder/{token}"));
        assert_eq!(item.title, "песня");
        assert!(run(&files, "files/home/Документы/1.txt", "play", json!({})).await.is_err(), "not a sound");
    }

    #[tokio::test]
    async fn a_replaced_file_of_the_household_waits_in_the_bin() {
        let (files, _dir) = made(false);
        run(&files, "files/home/Документы/a.txt", "write", json!({"text": "старое"})).await.unwrap();
        run(&files, "files/home/Документы/a.txt", "write", json!({"text": "новое"})).await.unwrap();
        let read = run(&files, "files/home/Документы/a.txt", "read", json!({"as": "text"})).await.unwrap();
        assert_eq!(read["text"], "новое");
        let bin = files.places.home_bin();
        let kept: Vec<String> = std::fs::read_dir(&bin)
            .unwrap()
            .flatten()
            .map(|dir| std::fs::read_to_string(dir.path().join("a.txt")).unwrap())
            .collect();
        assert_eq!(kept, ["старое"]);
        // An app's own folder keeps no bin.
        run(&files, "files/app/paint/b.txt", "write", json!({"text": "1"})).await.unwrap();
        run(&files, "files/app/paint/b.txt", "write", json!({"text": "2"})).await.unwrap();
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
    }

    #[test]
    fn names_go_as_people_count() {
        let mut names = vec!["Фото 10", "фото 2", "Фото 1", "a", "B"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["a", "B", "Фото 1", "фото 2", "Фото 10"]);
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
