//! Where our media comes from: every source behind one interface (`MediaSource`) — a radio list in a JSON file,
//! audio files in a folder, a plugin's catalog — kept by kind in `MediaSources`, and how a query matches them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use rand::seq::SliceRandom;
use serde_json::Value;

use super::duration::audio_duration_ms;
use super::media_types::{MediaItem, MediaKind, MediaSource};
use crate::skills::js;
use crate::skills::text::normalize;

/// Every source of media by kind, in the order they joined: ours (built at the start) first, then the plugins' as
/// they are switched on. The media skills read it on every request, so a source that comes or goes applies at once.
#[derive(Default)]
pub struct MediaSources {
    sources: RwLock<Vec<Arc<dyn MediaSource>>>,
}

impl MediaSources {
    pub fn new(sources: Vec<Arc<dyn MediaSource>>) -> Arc<MediaSources> {
        Arc::new(MediaSources { sources: RwLock::new(sources) })
    }

    /// A source from now on, in place of one with its id.
    pub fn add(&self, source: Arc<dyn MediaSource>) {
        let mut sources = self.sources.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = source.id();
        match sources.iter().position(|known| known.id() == id) {
            Some(index) => sources[index] = source,
            None => sources.push(source),
        }
    }

    /// The sources whose id starts so (a plugin's: `plugin:radio/`) leave.
    pub fn remove_prefixed(&self, prefix: &str) {
        self.sources
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|source| !source.id().starts_with(prefix));
    }

    /// The sources of a kind, in order.
    pub fn of(&self, kind: MediaKind) -> Vec<Arc<dyn MediaSource>> {
        let sources = self.sources.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let of_kind: Vec<Arc<dyn MediaSource>> =
            sources.iter().filter(|source| source.kind() == kind).cloned().collect();
        // A source that is only a fallback (our radio list, now that «Радио» is a built-in plugin — stage 10 of
        // docs/plans/plugins.md) yields to any other of its kind that has something to play.
        if of_kind.iter().any(|source| !source.fallback() && source.available()) {
            return of_kind.into_iter().filter(|source| !source.fallback()).collect();
        }
        of_kind
    }

    /// A source by its id (`content:music`).
    pub fn by_id(&self, id: &str) -> Option<Arc<dyn MediaSource>> {
        self.sources
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|source| source.id() == id)
            .cloned()
    }

    /// Whether any source of the kind has something to play.
    pub fn available(&self, kind: MediaKind) -> bool {
        self.of(kind).iter().any(|source| source.available())
    }

    /// The first source of the kind that has something for the query, with what it found.
    pub async fn find(
        &self,
        kind: MediaKind,
        query: Option<String>,
        media_base: Option<String>,
    ) -> Option<(Arc<dyn MediaSource>, Vec<MediaItem>)> {
        for source in self.of(kind).into_iter().filter(|source| source.available()) {
            let found = source.search(query.clone(), media_base.clone()).await;
            if !found.is_empty() {
                return Some((source, found));
            }
        }
        None
    }

    /// Everything the sources of a kind would play, one after another.
    pub async fn all(&self, kind: MediaKind, media_base: Option<String>) -> Vec<MediaItem> {
        let mut items = Vec::new();
        for source in self.of(kind).into_iter().filter(|source| source.available()) {
            items.extend(source.search(None, media_base.clone()).await);
        }
        items
    }
}

const STOPWORDS: &[&str] = &[
    "в", "во", "на", "у", "с", "со", "к", "ко", "по", "и", "за", "из", "от", "до", "для", "про", "о", "об", "мне",
    "мой", "моя",
];

/// Words of a query found in a text: the share of the query's words, 0–1.
pub fn match_score(query: &str, text: &str) -> f64 {
    let query = normalize(query);
    let words: Vec<&str> = query.split(' ').filter(|word| !word.is_empty() && !STOPWORDS.contains(word)).collect();
    if words.is_empty() {
        return 0.0;
    }
    let haystack = normalize(text);
    // A word matches by its stem, so "студию" finds "студия" and "маме" finds "мама".
    let stem = |word: &str| -> String {
        let chars: Vec<char> = word.chars().collect();
        let keep = match chars.len() {
            n if n > 4 => n - 2,
            4 => 3,
            n => n,
        };
        chars[..keep].iter().collect()
    };
    let found = words.iter().filter(|word| haystack.contains(&stem(word))).count();
    found as f64 / words.len() as f64
}

/// Radio stations from a JSON file (CONTENT_DIR/radio.json): `[{id, title, aliases, url, format}]`. Each station is
/// played from its own stream, as Yandex plays radio (HLS on the station's server). Read on every search, so edits
/// apply at once.
pub struct StaticRadioCatalog {
    file: PathBuf,
}

struct RadioEntry {
    item: MediaItem,
    names: String,
}

impl StaticRadioCatalog {
    pub fn new(file: PathBuf) -> StaticRadioCatalog {
        StaticRadioCatalog { file }
    }

    fn read(&self) -> Vec<RadioEntry> {
        let Ok(text) = std::fs::read_to_string(&self.file) else { return Vec::new() };
        let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
        entries
            .iter()
            .filter_map(|entry| {
                let field = |key: &str| entry.get(key).and_then(Value::as_str).filter(|value| !value.is_empty());
                let (id, url, title) = (field("id")?, field("url")?, field("title")?);
                let aliases: Vec<&str> = entry
                    .get("aliases")
                    .and_then(Value::as_array)
                    .map(|aliases| aliases.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let format = match entry.get("format").and_then(Value::as_str) {
                    Some(format) => format.to_string(),
                    None if url.contains(".m3u8") => "HLS".into(),
                    None => "MP3".into(),
                };
                let mut names = vec![title];
                names.extend(aliases);
                Some(RadioEntry {
                    item: MediaItem {
                        id: id.into(),
                        title: title.into(),
                        url: url.into(),
                        format,
                        codec: None,
                        r#type: "FmRadio".into(),
                        subtitle: None,
                        image_url: entry.get("imageUrl").and_then(Value::as_str).map(str::to_string),
                        duration_ms: None,
                    },
                    names: names.join(" "),
                })
            })
            .collect()
    }

    fn find(&self, query: Option<&str>) -> Vec<MediaItem> {
        let stations = self.read();
        let Some(query) = query.filter(|query| !query.is_empty()) else {
            return stations.into_iter().map(|station| station.item).collect();
        };
        let mut scored: Vec<(f64, MediaItem)> = stations
            .into_iter()
            .map(|station| (match_score(query, &station.names), station.item))
            .filter(|(score, _)| *score >= 0.5)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().map(|(_, item)| item).collect()
    }
}

impl MediaSource for StaticRadioCatalog {
    fn id(&self) -> String {
        "content:radio".into()
    }

    fn kind(&self) -> MediaKind {
        MediaKind::Radio
    }

    fn search(&self, query: Option<String>, _media_base: Option<String>) -> BoxFuture<'_, Vec<MediaItem>> {
        Box::pin(async move { self.find(query.as_deref()) })
    }

    fn available(&self) -> bool {
        !self.read().is_empty()
    }

    /// «Радио» is a built-in plugin now (stage 10): our list of content/radio.json answers only when it is off.
    fn fallback(&self) -> bool {
        true
    }
}

/// Codecs as Yandex names them in audio_play, where it is known; the format of a file is always MP3.
fn codec(extension: &str) -> Option<&'static str> {
    (extension == "m4a").then_some("AacMp4")
}

/// The audio extension of a file name, lower case, if it is one of ours.
fn audio_extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    ["mp3", "ogg", "opus", "m4a", "aac", "flac", "wav"].contains(&extension.as_str()).then_some(extension)
}

/// Audio files under a folder, served by our /media: `Artist - Title.mp3`, folders as albums.
#[derive(Clone)]
pub struct FolderLibrary {
    /// CONTENT_DIR.
    root: PathBuf,
    /// The folder under it: music, tales.
    folder: String,
    r#type: String,
}

impl FolderLibrary {
    /// Its items are of this audio_play type: tracks are music, the rest (FairyTale, AudioBook) tales.
    pub fn new(root: PathBuf, folder: &str, r#type: &str) -> FolderLibrary {
        FolderLibrary { root, folder: folder.into(), r#type: r#type.into() }
    }

    /// The items for this query, searched off the runtime's threads: a folder of files is read with their headers.
    async fn items(&self, query: Option<String>, media_base: Option<String>) -> Vec<MediaItem> {
        let Some(base) = media_base.filter(|base| !base.is_empty()) else { return Vec::new() };
        let library = self.clone();
        let search = move || library.search_now(query.as_deref(), &base);
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::spawn_blocking(search).await.unwrap_or_default(),
            Err(_) => search(),
        }
    }

    fn search_now(&self, query: Option<&str>, base: &str) -> Vec<MediaItem> {
        let mut items = self.scan(base);
        let Some(query) = query.filter(|query| !query.is_empty()) else {
            items.shuffle(&mut rand::rng());
            return items;
        };
        let mut scored: Vec<(f64, MediaItem)> = items
            .into_iter()
            .map(|item| {
                (match_score(query, &format!("{} {}", item.title, item.subtitle.as_deref().unwrap_or(""))), item)
            })
            .filter(|(score, _)| *score >= 0.5)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().map(|(_, item)| item).collect()
    }

    fn scan(&self, base: &str) -> Vec<MediaItem> {
        let mut items = Vec::new();
        self.walk(&self.root.join(&self.folder), 0, base, &mut items);
        items
    }

    fn walk(&self, current: &Path, depth: usize, base: &str, items: &mut Vec<MediaItem>) {
        let Ok(entries) = std::fs::read_dir(current) else { return };
        let mut names: Vec<String> = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            if name.starts_with('.') {
                continue;
            }
            let full = current.join(&name);
            let Ok(meta) = std::fs::metadata(&full) else { continue };
            if meta.is_dir() && depth < 4 {
                self.walk(&full, depth + 1, base, items);
            }
            let Some(extension) = audio_extension(&name).filter(|_| meta.is_file()) else { continue };
            let Ok(relative) = full.strip_prefix(&self.root) else { continue };
            let parts: Vec<String> =
                relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            let stem = &name[..name.len() - extension.len() - 1];
            // `Artist - Title`: the artist, then the title up to any further « - ».
            let mut halves = stem.split(" - ");
            let (artist, title) = if stem.contains(" - ") {
                let artist = halves.next().unwrap_or("");
                (Some(artist), halves.next().unwrap_or(""))
            } else {
                (None, stem)
            };
            let album = current.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            items.push(MediaItem {
                id: parts.join("/"),
                title: js::trim(title).to_string(),
                subtitle: Some(artist.map(|artist| js::trim(artist).to_string()).unwrap_or(album)),
                url: format!(
                    "{base}/{}",
                    parts.iter().map(|part| js::encode_uri_component(part)).collect::<Vec<_>>().join("/")
                ),
                format: "MP3".into(),
                codec: codec(&extension).map(str::to_string),
                r#type: self.r#type.clone(),
                image_url: None,
                duration_ms: audio_duration_ms(&full),
            });
        }
    }

    /// Whether any audio file is under the folder.
    fn has_audio(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return false;
            }
            let Ok(meta) = std::fs::metadata(entry.path()) else { return false };
            (meta.is_dir() && depth < 4 && FolderLibrary::has_audio(&entry.path(), depth + 1))
                || (meta.is_file() && audio_extension(&name).is_some())
        })
    }
}

impl MediaSource for FolderLibrary {
    fn id(&self) -> String {
        format!("content:{}", self.folder)
    }

    fn kind(&self) -> MediaKind {
        if self.r#type == "Track" {
            MediaKind::Music
        } else {
            MediaKind::Tales
        }
    }

    /// «Своя музыка» and «Сказки» are built-in plugins now (stage 10): the folder answers by itself only when they are
    /// off; they read the same folder through `host.content`.
    fn fallback(&self) -> bool {
        true
    }

    fn search(&self, query: Option<String>, media_base: Option<String>) -> BoxFuture<'_, Vec<MediaItem>> {
        Box::pin(self.items(query, media_base))
    }

    fn available(&self) -> bool {
        FolderLibrary::has_audio(&self.root.join(&self.folder), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_words_and_their_stems() {
        assert_eq!(match_score("студию 21", "Студия 21 studio 21"), 1.0);
        assert_eq!(match_score("джаз", "Студия 21"), 0.0);
        assert_eq!(match_score("маме", "Мама"), 1.0);
        assert_eq!(match_score("в на", "Мама"), 0.0);
    }

    #[tokio::test]
    async fn lists_a_folder_as_artists_and_albums_with_links_to_media() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("music/Альбом")).unwrap();
        std::fs::write(dir.path().join("music/Альбом/Кино - Группа крови.mp3"), "ID3").unwrap();
        std::fs::write(dir.path().join("music/Без исполнителя.M4A"), "x").unwrap();
        std::fs::write(dir.path().join("music/A - B - C.mp3"), "x").unwrap();
        std::fs::write(dir.path().join("music/.hidden.mp3"), "x").unwrap();
        std::fs::write(dir.path().join("music/notes.txt"), "x").unwrap();
        let library = FolderLibrary::new(dir.path().to_path_buf(), "music", "Track");
        assert!(library.available());
        let mut items = library.search(None, Some("http://gw/media".into())).await;
        items.sort_by(|a, b| a.id.cmp(&b.id));
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["music/A - B - C.mp3", "music/Альбом/Кино - Группа крови.mp3", "music/Без исполнителя.M4A"]);
        assert_eq!((items[0].title.as_str(), items[0].subtitle.as_deref()), ("B", Some("A")));
        assert_eq!((items[1].title.as_str(), items[1].subtitle.as_deref()), ("Группа крови", Some("Кино")));
        assert_eq!((items[2].title.as_str(), items[2].subtitle.as_deref()), ("Без исполнителя", Some("music")));
        assert_eq!(items[2].codec.as_deref(), Some("AacMp4"));
        assert!(library.search(None, None).await.is_empty());
        let empty = FolderLibrary::new(dir.path().to_path_buf(), "tales", "FairyTale");
        assert!(!empty.available());
        assert_eq!((library.kind(), empty.kind()), (MediaKind::Music, MediaKind::Tales));
    }

    /// A catalog of stations by name, as a plugin's would be.
    struct Catalog(&'static str, Vec<&'static str>);

    impl MediaSource for Catalog {
        fn id(&self) -> String {
            self.0.into()
        }
        fn kind(&self) -> MediaKind {
            MediaKind::Radio
        }
        fn search(&self, query: Option<String>, _: Option<String>) -> BoxFuture<'_, Vec<MediaItem>> {
            let items = self
                .1
                .iter()
                .filter(|title| query.as_deref().is_none_or(|query| match_score(query, title) >= 0.5))
                .map(|title| MediaItem {
                    id: format!("{}/{title}", self.0),
                    title: title.to_string(),
                    url: format!("https://radio.example/{title}"),
                    format: "MP3".into(),
                    codec: None,
                    r#type: "FmRadio".into(),
                    subtitle: None,
                    image_url: None,
                    duration_ms: None,
                })
                .collect();
            Box::pin(async move { items })
        }
        fn available(&self) -> bool {
            !self.1.is_empty()
        }
    }

    #[tokio::test]
    async fn asks_the_sources_of_a_kind_in_order_and_lets_one_come_and_go() {
        let sources = MediaSources::new(vec![
            Arc::new(Catalog("content:radio", vec!["Студия 21"])),
            Arc::new(Catalog("content:empty", vec![])),
        ]);
        sources.add(Arc::new(Catalog("plugin:radio/stations", vec!["Джаз", "Студия 21 плюс"])));
        let (source, found) = sources.find(MediaKind::Radio, Some("студия".into()), None).await.unwrap();
        assert_eq!((source.id(), found[0].title.as_str()), ("content:radio".into(), "Студия 21"));
        let (source, _) = sources.find(MediaKind::Radio, Some("джаз".into()), None).await.unwrap();
        assert_eq!(source.id(), "plugin:radio/stations");
        assert!(sources.find(MediaKind::Music, None, None).await.is_none());
        assert_eq!(sources.all(MediaKind::Radio, None).await.len(), 3);
        sources.add(Arc::new(Catalog("plugin:radio/stations", vec!["Рок"])));
        assert_eq!(sources.all(MediaKind::Radio, None).await.len(), 2, "the same id is replaced");
        sources.remove_prefixed("plugin:radio/");
        assert!(sources.find(MediaKind::Radio, Some("рок".into()), None).await.is_none());
        assert!(sources.available(MediaKind::Radio));
    }
}
