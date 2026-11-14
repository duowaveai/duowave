//! The one engine that shows and plays photos, videos and sounds (docs/plans/feed.md, «Один движок показа»). Every
//! file a screen gets — the panel's viewer and «Папки», an app's window and its cards, the computer's screen, the
//! station's player — goes out here, whoever asked for it:
//!
//! - **A short address with a key**, `/media/v/<key>`: the bus checks who asks (a person in the panel; an app's window
//!   or card for the person looking at it; an app asking the station to play — the letter `media` or `play` of the
//!   folders) and the engine gives a key to that one file: 24 random bytes, no path in it, for six hours. The same
//!   file for the same asker keeps its address while more than half its life is left, so the browser's cache holds.
//!   A key given to an app closes with the app. A key for a folder the household keeps at home (no «и вне дома»), and
//!   every key for the station's player, opens on the home network only. A restart forgets the keys: the panel asks
//!   again.
//! - **Seeking**: Range, from the disk as it is — an episode of 70 MB never goes whole into memory.
//! - **The type by the content**, not by the name (kind.rs), and never anything a browser runs: a key opens a photo, a
//!   video or a sound only; a package's own files anything but HTML, SVG and scripts.
//! - **Pictures of it**: `?size=<px>` — a photo made small (turned by its EXIF; HEIC as JPEG), a video's cover frame;
//!   and how long a sound or a video lasts, read from its own header (probe.rs).
//!
//! The older addresses are thin doors to it (mod.rs): `/media/folder/<key>` (the same keys, as the station's player
//! and the computer's screen had them), `/media/<path>` (the household's music and tales for the station),
//! `/media/packages/<id>/<path>` (a package's avatars, samples, sounds).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::kind::{self, Kind, Sniffed};
use super::probe;
use crate::bus::providers::files::preview;
use crate::platform::http::ApiError;

/// How long a key holds.
pub const KEY_LIFE: Duration = Duration::from_secs(6 * 3600);
/// Keys kept at most: past them, the ones ending soonest go.
const KEYS_MOST: usize = 50_000;
/// The ended keys are swept at most this often.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// The keys given and the pictures made.
pub struct MediaEngine {
    keys: Mutex<Keys>,
    /// Where the photos made small and the covers are kept (`folder-previews`).
    previews: PathBuf,
    /// Pictures made at once: a page of a folder asks for a hundred, and a 48 MP photo takes ~150 MB to decode. One on
    /// the station, up to four on a computer.
    making: tokio::sync::Semaphore,
}

#[derive(Default)]
struct Keys {
    by_key: HashMap<String, Ticket>,
    /// The key a file has for an asker: (the file, the app, home only).
    by_file: HashMap<(PathBuf, Option<String>, bool), String>,
    swept: Option<Instant>,
}

/// What a key opens.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub path: PathBuf,
    pub sniffed: Sniffed,
    pub title: String,
    /// Given to an app (its window, its card, its sound for the station): it closes when the app is gone.
    pub app: Option<String>,
    /// Opens on the home network only.
    pub home_only: bool,
    until: Instant,
}

/// To whom a key is given.
#[derive(Debug, Clone, Default)]
pub struct Grant {
    pub app: Option<String>,
    pub home_only: bool,
}

/// A key given, as the screens read it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// The key itself: the station's player and the computer's screen keep it as their `file`.
    pub key: String,
    /// `/media/v/<key>` on this gateway.
    pub url: String,
    pub kind: Kind,
    pub mime: &'static str,
    /// Whether browsers show it as it is: a HEIC photo is shown made small (`?size=`).
    pub shown: bool,
    pub title: String,
    pub size: u64,
    /// How long it plays, in seconds — when asked for (`probe`) and its header says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Its life left, in seconds.
    pub expires_in_s: u64,
}

impl Link {
    /// What its first bytes said, as the engine read them when it gave the key.
    pub fn sniffed(&self) -> Sniffed {
        Sniffed { kind: self.kind, mime: self.mime, shown: self.shown }
    }
}

impl MediaEngine {
    pub fn new(previews: PathBuf, station: bool) -> MediaEngine {
        let making = if station { 1 } else { std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4) };
        MediaEngine { keys: Mutex::new(Keys::default()), previews, making: tokio::sync::Semaphore::new(making) }
    }

    pub fn previews(&self) -> &Path {
        &self.previews
    }

    fn keys(&self) -> std::sync::MutexGuard<'_, Keys> {
        self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A key to one file — a photo, a video or a sound by its bytes; else why not, in words.
    pub fn give(&self, path: &Path, grant: Grant) -> Result<Link, String> {
        let meta = std::fs::metadata(path).map_err(|_| crate::tr!("файла нет"))?;
        if !meta.is_file() {
            return Err(crate::tr!("это папка, а не файл"));
        }
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let sniffed = sniff_file(path)
            .ok_or_else(|| crate::tr!("«{name}» — не фото, не видео и не звук", name = name.clone()))?;
        let title = name.rsplit_once('.').map(|(stem, _)| stem.to_string()).filter(|stem| !stem.is_empty());
        let title = title.unwrap_or(name);
        let now = Instant::now();
        let mut keys = self.keys();
        keys.sweep(now);
        let file = (path.to_path_buf(), grant.app.clone(), grant.home_only);
        let kept = keys
            .by_file
            .get(&file)
            .and_then(|key| keys.by_key.get(key).map(|ticket| (key.clone(), ticket.until)))
            .filter(|(_, until)| until.saturating_duration_since(now) > KEY_LIFE / 2);
        let (key, until) = match kept {
            Some(kept) => kept,
            None => {
                let key = crate::utils::ids::random_hex(24);
                let until = now + KEY_LIFE;
                let ticket = Ticket {
                    path: path.to_path_buf(),
                    sniffed,
                    title: title.clone(),
                    app: grant.app,
                    home_only: grant.home_only,
                    until,
                };
                keys.by_key.insert(key.clone(), ticket);
                keys.by_file.insert(file, key.clone());
                (key, until)
            }
        };
        Ok(Link {
            url: format!("/media/v/{key}"),
            key,
            kind: sniffed.kind,
            mime: sniffed.mime,
            shown: sniffed.shown,
            title,
            size: meta.len(),
            duration: None,
            expires_in_s: until.saturating_duration_since(now).as_secs(),
        })
    }

    /// What a key opens, while it holds.
    pub fn ticket(&self, key: &str) -> Option<Ticket> {
        let now = Instant::now();
        self.keys().by_key.get(key).filter(|ticket| ticket.until > now).cloned()
    }

    /// The keys given to an app close: it was removed.
    pub fn forget_app(&self, app: &str) {
        let mut keys = self.keys();
        keys.by_key.retain(|_, ticket| ticket.app.as_deref() != Some(app));
        let Keys { by_key, by_file, .. } = &mut *keys;
        by_file.retain(|_, key| by_key.contains_key(key));
    }

    /// A picture of it — a photo made small, a video's cover frame: a JPEG, its longer side `side` at most. One at a
    /// time per core, off the async threads.
    pub async fn picture(&self, path: PathBuf, sniffed: Sniffed, side: u32) -> Result<Vec<u8>, String> {
        let _turn = self.making.acquire().await.map_err(|error| error.to_string())?;
        let kept = self.previews.clone();
        tokio::task::spawn_blocking(move || picture_now(&path, sniffed, side, &kept))
            .await
            .map_err(|error| error.to_string())?
    }
}

impl Keys {
    /// The ended keys go, once a minute; past the most kept, the ones ending soonest.
    fn sweep(&mut self, now: Instant) {
        let due = self.swept.is_none_or(|at| now.duration_since(at) >= SWEEP_EVERY);
        if !due && self.by_key.len() < KEYS_MOST {
            return;
        }
        self.swept = Some(now);
        self.by_key.retain(|_, ticket| ticket.until > now);
        if self.by_key.len() >= KEYS_MOST {
            let mut ends: Vec<(Instant, String)> =
                self.by_key.iter().map(|(key, ticket)| (ticket.until, key.clone())).collect();
            ends.sort();
            for (_, key) in ends.into_iter().take(self.by_key.len() - KEYS_MOST * 9 / 10) {
                self.by_key.remove(&key);
            }
        }
        let by_key = &self.by_key;
        self.by_file.retain(|_, key| by_key.contains_key(key));
    }
}

/// A picture of a file right now: a photo made small, a video's cover frame. The letter `preview` of the folders
/// answers in place with it; the engine's own route waits its turn (`MediaEngine::picture`).
pub fn picture_now(path: &Path, sniffed: Sniffed, side: u32, kept: &Path) -> Result<Vec<u8>, String> {
    match sniffed.kind {
        Kind::Image => preview::preview(path, side, kept),
        Kind::Video => probe::cover(path, sniffed, side, kept),
        Kind::Audio => Err(crate::tr!("у звука нет картинки")),
    }
}

/// A file's first bytes, and its name.
fn head_of(path: &Path) -> Option<(Vec<u8>, String)> {
    let mut head = Vec::with_capacity(kind::HEAD);
    std::fs::File::open(path).ok()?.take(kind::HEAD as u64).read_to_end(&mut head).ok()?;
    let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    Some((head, name))
}

/// What a file is by its first bytes: a photo, a video, a sound — or none.
pub fn sniff_file(path: &Path) -> Option<Sniffed> {
    let (head, name) = head_of(path)?;
    kind::sniff(&head, &name)
}

/// How a file may go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allow {
    /// A photo, a video or a sound, by its bytes; nothing else.
    Media,
    /// A package's own file: anything but what a browser runs; a photo, a video or a sound gets its type by its bytes.
    Assets,
}

/// A file of the disk to whoever asked: with Range, its type by its bytes, nothing a browser runs. `cache` is its
/// Cache-Control.
pub async fn serve(path: PathBuf, allow: Allow, request: Request, cache: &'static str) -> Result<Response, ApiError> {
    let looked = path.clone();
    let (head, name) = tokio::task::spawn_blocking(move || head_of(&looked))
        .await
        .map_err(|error| ApiError::internal(format!("media: {error}")))?
        .ok_or_else(|| ApiError::not_found("Not Found"))?;
    let sniffed = kind::sniff(&head, &name);
    let refused = kind::runnable(&head, &name) || (allow == Allow::Media && sniffed.is_none());
    if refused {
        return Err(ApiError::not_found("Not Found"));
    }
    let mut response = match ServeFile::new(&path).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(error) => return Err(ApiError::internal(format!("media: {error}"))),
    };
    if let Some(sniffed) = sniffed.filter(|_| response.status().is_success()) {
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(sniffed.mime));
    }
    safe(response.headers_mut(), cache);
    Ok(response)
}

/// A picture made as the answer.
pub fn jpeg(bytes: Vec<u8>, cache: &'static str) -> Response {
    let mut response = bytes.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    safe(response.headers_mut(), cache);
    response
}

/// What every answer of the engine carries: its type is final, it runs nothing even opened as a page, its cache.
fn safe(headers: &mut HeaderMap, cache: &'static str) {
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];

    #[test]
    fn gives_one_file_a_key_that_holds_and_keeps_its_address() {
        let dir = tempfile::tempdir().unwrap();
        let engine = MediaEngine::new(dir.path().join("previews"), false);
        let photo = dir.path().join("кот.jpg");
        std::fs::write(&photo, JPEG).unwrap();
        let link = engine.give(&photo, Grant::default()).unwrap();
        assert_eq!((link.kind, link.mime, link.title.as_str()), (Kind::Image, "image/jpeg", "кот"));
        assert_eq!(link.key.len(), 48);
        assert_eq!(link.url, format!("/media/v/{}", link.key));
        assert_eq!(engine.ticket(&link.key).unwrap().path, photo);
        // The same file for the same asker: the same address, so the browser's cache holds.
        assert_eq!(engine.give(&photo, Grant::default()).unwrap().key, link.key);
        // For an app, or for home only: a key of its own.
        let app = Grant { app: Some("feed".into()), home_only: false };
        let for_app = engine.give(&photo, app.clone()).unwrap();
        assert_ne!(for_app.key, link.key);
        assert_eq!(engine.ticket(&for_app.key).unwrap().app.as_deref(), Some("feed"));
        let home = engine.give(&photo, Grant { app: None, home_only: true }).unwrap();
        assert!(engine.ticket(&home.key).unwrap().home_only);
        // An app removed: its keys close, the others hold.
        engine.forget_app("feed");
        assert!(engine.ticket(&for_app.key).is_none());
        assert!(engine.ticket(&link.key).is_some());
        assert!(engine.ticket(&"0".repeat(48)).is_none());
    }

    #[test]
    fn gives_no_key_to_what_is_not_a_photo_a_video_or_a_sound() {
        let dir = tempfile::tempdir().unwrap();
        let engine = MediaEngine::new(dir.path().join("previews"), false);
        let page = dir.path().join("кот.jpg");
        std::fs::write(&page, b"<html><script>alert(1)</script>").unwrap();
        assert!(engine.give(&page, Grant::default()).unwrap_err().contains("не фото"), "a page named a photo");
        assert!(engine.give(dir.path(), Grant::default()).is_err(), "a folder");
        assert!(engine.give(&dir.path().join("нет.mp4"), Grant::default()).is_err());
    }

    fn asked(range: Option<&str>) -> Request {
        let mut request = Request::builder().uri("/media/v/key");
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        request.body(axum::body::Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn serves_with_seeking_its_type_by_its_bytes_and_nothing_that_runs() {
        let dir = tempfile::tempdir().unwrap();
        // A JPEG named a PNG goes as a JPEG; a part of it as asked, with its range.
        let photo = dir.path().join("кот.png");
        let mut bytes = JPEG.to_vec();
        bytes.resize(1000, 7);
        std::fs::write(&photo, &bytes).unwrap();
        let whole = serve(photo.clone(), Allow::Media, asked(None), "no-store").await.unwrap();
        assert_eq!(whole.status(), axum::http::StatusCode::OK);
        assert_eq!(whole.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(whole.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(whole.headers()[header::CONTENT_SECURITY_POLICY], "sandbox; default-src 'none'");
        let part = serve(photo, Allow::Media, asked(Some("bytes=100-199")), "no-store").await.unwrap();
        assert_eq!(part.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 100-199/1000");
        assert_eq!(part.headers()[header::CONTENT_TYPE], "image/jpeg");
        // A page named a photo: never, as media or as a package's file.
        let page = dir.path().join("кот.jpg");
        std::fs::write(&page, b"<html><script>alert(1)</script>").unwrap();
        assert!(serve(page.clone(), Allow::Media, asked(None), "no-store").await.is_err());
        assert!(serve(page, Allow::Assets, asked(None), "no-store").await.is_err());
        let svg = dir.path().join("logo.svg");
        std::fs::write(&svg, b"\x89PNG\r\n\x1a\n").unwrap();
        assert!(serve(svg, Allow::Assets, asked(None), "no-store").await.is_err(), "a runnable name");
        // A package's font: neither media nor runnable — it goes.
        let font = dir.path().join("font.woff2");
        std::fs::write(&font, b"wOF2\0\x01\0\0").unwrap();
        assert!(serve(font.clone(), Allow::Assets, asked(None), "no-store").await.is_ok());
        assert!(serve(font, Allow::Media, asked(None), "no-store").await.is_err(), "a key opens media only");
    }

    #[test]
    fn sweeps_ended_keys_and_keeps_within_the_most() {
        let mut keys = Keys::default();
        let now = Instant::now();
        let ticket = |until: Instant| Ticket {
            path: PathBuf::from("a"),
            sniffed: kind::sniff(JPEG, "a").unwrap(),
            title: "a".into(),
            app: None,
            home_only: false,
            until,
        };
        keys.by_key.insert("old".into(), ticket(now));
        keys.by_key.insert("new".into(), ticket(now + KEY_LIFE));
        keys.by_file.insert((PathBuf::from("a"), None, false), "old".into());
        keys.sweep(now + Duration::from_secs(1));
        assert!(!keys.by_key.contains_key("old") && keys.by_key.contains_key("new"));
        assert!(keys.by_file.is_empty(), "the file's key went with it");
    }
}
