//! The packages' catalog (docs/plans/characters.md, sections 2 and 4): one feed for every kind of package —
//! characters first — as static files on our site, read when the panel opens the catalog and not more often than
//! every 6 hours, with its ETag. Its last good copy and the previews it names live in `DATA_DIR/packages-catalog/`:
//! without the network the panel sees that copy, and the site never learns who looks at what. A feed that does not
//! parse leaves the copy before it in place and says why.
//!
//! The feed comes from PACKAGES_FEED_URL, else the one built in at release (DUOWAVE_PACKAGES_FEED); `off` keeps the
//! gateway off the network for it, and so does the owner's switch (setting `packages_catalog`). HTTPS only (HTTP for
//! this computer, to try a catalog), and every file it names from the same origin. Nothing installs by itself: a
//! person with the right presses the button. Until the catalog is signed (stage 8), plugins and apps in it are listed
//! and never installed — only packages without code.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::controller::{PackageView, PlacementNow, PluginHost};
use super::install::{self, Expected, MAX_ARCHIVE};
use super::manifest::{valid_id, PackageKind, HOST_API, MAX_LICENSE};
use super::registry;
use crate::config::PluginsConfig;
use crate::core::availability::Placement;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::settings::definitions::PACKAGES_CATALOG;
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// The feed built into a release; none in a local build.
const BUILT_IN_FEED: Option<&str> = option_env!("DUOWAVE_PACKAGES_FEED");
/// The feed format this gateway reads.
pub const FEED_FORMAT: u64 = 1;
/// The feed is read again after this long.
const FRESH: Duration = Duration::from_secs(6 * 3600);
const FEED_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FEED: u64 = 1024 * 1024;
/// A preview: the picture and up to three phrases.
const MAX_IMAGE: u64 = 256 * 1024;
const MAX_SAMPLE: u64 = 64 * 1024;
const MAX_SAMPLES: usize = 3;
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(120);

/// Where the feed is, as the configuration and the build say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedSource {
    /// PACKAGES_FEED_URL=off.
    Off,
    /// Neither the environment nor the build names one.
    Missing,
    Url(String),
}

impl FeedSource {
    pub fn of(configured: Option<&str>, built_in: Option<&str>) -> FeedSource {
        match configured.map(str::trim).filter(|url| !url.is_empty()) {
            Some(url) if url.eq_ignore_ascii_case("off") => FeedSource::Off,
            Some(url) => FeedSource::Url(url.to_string()),
            None => match built_in.map(str::trim).filter(|url| !url.is_empty()) {
                Some(url) => FeedSource::Url(url.to_string()),
                None => FeedSource::Missing,
            },
        }
    }
}

/// A feed's address the gateway may read: HTTPS, or HTTP on this computer (a catalog being tried).
pub fn feed_url(text: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(text).map_err(|_| format!("адрес каталога «{text}» — не адрес"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if local => Ok(url),
        _ => Err("каталог — только по HTTPS (HTTP — только с localhost и 127.0.0.1)".into()),
    }
}

/// A file the feed names, from the feed's own origin (scheme, host, port); None when it is anywhere else.
pub fn same_origin(base: &url::Url, reference: &str) -> Option<url::Url> {
    let url = base.join(reference).ok()?;
    (url.origin() == base.origin()).then_some(url)
}

/// A package in the feed, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub kind: PackageKind,
    pub version: semver::Version,
    pub title: String,
    pub description: Option<String>,
    pub api: String,
    pub runs: Vec<Placement>,
    pub min_gateway: Option<semver::Version>,
    pub url: url::Url,
    pub sha256: String,
    pub size: u64,
    pub image: Option<url::Url>,
    pub samples: Vec<url::Url>,
    /// What it will need (an engine, room): the same language as a voice's `needs`, as the feed gives it.
    pub needs: Value,
    pub license: Option<String>,
    pub verified: bool,
    /// Its `budget.memoryMb`, as the feed tells it: on the station, what does not fit is shown as not going here.
    pub memory_mb: Option<u64>,
    /// An app's archives by platform (`platforms`): the station takes `linux-aarch64`'s, a Mac `darwin-arm64`'s.
    pub platforms: BTreeMap<String, Archive>,
}

/// An archive of the feed: its address, its hash, its size.
#[derive(Debug, Clone, PartialEq)]
pub struct Archive {
    pub url: url::Url,
    pub sha256: String,
    pub size: u64,
}

impl Entry {
    /// The archive for this machine: its platform's, else the one archive.
    pub fn archive(&self) -> Archive {
        self.platforms.get(&super::manifest::platform()).cloned().unwrap_or_else(|| Archive {
            url: self.url.clone(),
            sha256: self.sha256.clone(),
            size: self.size,
        })
    }
}

/// The feed, checked: its packages, and what of it was left out and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub updated_at: Option<String>,
    pub packages: Vec<Entry>,
    pub skipped: Vec<String>,
    /// Signed with the catalog's key (`catalog.json.minisig`): code may come from it (stage 8).
    pub signed: bool,
}

/// The catalog's signature: the feed's own address and `.minisig`, from the same origin.
pub const SIGNATURE_SUFFIX: &str = ".minisig";
const MAX_SIGNATURE: u64 = 8 * 1024;

impl Feed {
    /// The newest version of each package, in the feed's order.
    pub fn newest(&self) -> Vec<&Entry> {
        let mut seen: Vec<&Entry> = Vec::new();
        for entry in &self.packages {
            match seen.iter_mut().find(|known| known.id == entry.id) {
                Some(known) if known.version < entry.version => *known = entry,
                Some(_) => {}
                None => seen.push(entry),
            }
        }
        seen
    }

    /// A package by id: the version asked for, or its newest.
    pub fn find(&self, id: &str, version: Option<&str>) -> Option<&Entry> {
        match version {
            Some(version) => self.packages.iter().find(|entry| entry.id == id && entry.version.to_string() == version),
            None => self.newest().into_iter().find(|entry| entry.id == id),
        }
    }
}

fn entry(value: &Value, base: &url::Url) -> Result<Entry, String> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let id = text("id").filter(|id| valid_id(id)).ok_or("id: строчные латинские буквы, цифры и «-»")?.to_string();
    let at = |problem: &str| format!("{id}: {problem}");
    let kind = PackageKind::parse(text("kind").unwrap_or("plugin")).ok_or_else(|| at("kind: нет такого вида"))?;
    let version = text("version").and_then(|v| semver::Version::parse(v).ok()).ok_or_else(|| at("version"))?;
    let title: String = text("title").ok_or_else(|| at("нет title"))?.chars().take(80).collect();
    let description = text("description").map(|text| text.chars().take(500).collect());
    let api = text("api").unwrap_or("^1").to_string();
    semver::VersionReq::parse(&api).map_err(|_| at("api"))?;
    let runs: Vec<Placement> = match value.get("runs").and_then(Value::as_array) {
        Some(names) => names.iter().filter_map(|name| name.as_str().and_then(Placement::parse)).collect(),
        None => Placement::ALL.to_vec(),
    };
    let min_gateway = match text("minGateway") {
        Some(min) => Some(semver::Version::parse(min.trim_start_matches('v')).map_err(|_| at("minGateway"))?),
        None => None,
    };
    let url = text("url").and_then(|url| same_origin(base, url)).ok_or_else(|| at("url: архив с того же адреса"))?;
    let sha256 = text("sha256")
        .filter(|sha| sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| at("sha256: 64 шестнадцатеричных знака"))?
        .to_ascii_lowercase();
    let size = value.get("size").and_then(Value::as_u64).filter(|size| (1..=MAX_ARCHIVE).contains(size));
    let size = size.ok_or_else(|| at("size: размер архива, до 16 МБ"))?;
    let preview = value.get("preview");
    let image = preview.and_then(|preview| preview.get("image")).and_then(Value::as_str);
    let image = match image {
        Some(image) => Some(same_origin(base, image).ok_or_else(|| at("preview.image: с того же адреса"))?),
        None => None,
    };
    let mut samples = Vec::new();
    for sample in preview.and_then(|preview| preview.get("samples")).and_then(Value::as_array).into_iter().flatten() {
        let sample = sample.as_str().and_then(|sample| same_origin(base, sample));
        samples.push(sample.ok_or_else(|| at("preview.samples: с того же адреса"))?);
    }
    samples.truncate(MAX_SAMPLES);
    let needs = match value.get("needs") {
        None => Value::Array(Vec::new()),
        Some(Value::Array(needs)) if needs.iter().all(Value::is_object) => Value::Array(needs.clone()),
        Some(_) => return Err(at("needs: список")),
    };
    let license = text("license").map(|license| license.chars().take(MAX_LICENSE).collect());
    let verified = value.get("verified").and_then(Value::as_bool).unwrap_or(false);
    let memory_mb = value.pointer("/budget/memoryMb").and_then(Value::as_u64);
    let mut platforms = BTreeMap::new();
    for (key, archive) in value.get("platforms").and_then(Value::as_object).into_iter().flatten() {
        let url = archive.get("url").and_then(Value::as_str).and_then(|url| same_origin(base, url));
        let sha256 = archive
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|sha| sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()));
        let size = archive.get("size").and_then(Value::as_u64).filter(|size| (1..=MAX_ARCHIVE).contains(size));
        match (url, sha256, size) {
            (Some(url), Some(sha256), Some(size)) => {
                platforms.insert(key.clone(), Archive { url, sha256: sha256.to_ascii_lowercase(), size });
            }
            _ => return Err(at(&format!("platforms.{key}: url с того же адреса, sha256 и size"))),
        }
    }
    Ok(Entry {
        id,
        kind,
        version,
        title,
        description,
        api,
        runs,
        min_gateway,
        url,
        sha256,
        size,
        image,
        samples,
        needs,
        license,
        verified,
        memory_mb,
        platforms,
    })
}

/// Reads a feed by its schema: the whole of it must be one (format 1, a list of packages); a package in it that is
/// not one is left out with its reason.
pub fn parse_feed(text: &str, base: &url::Url) -> Result<Feed, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| format!("не JSON: {error}"))?;
    match value.get("format").and_then(Value::as_u64) {
        Some(FEED_FORMAT) => {}
        Some(other) => return Err(format!("format {other}: этот шлюз читает каталог версии {FEED_FORMAT}")),
        None => return Err("нет format".into()),
    }
    let list = value.get("packages").and_then(Value::as_array).ok_or("нет списка packages")?;
    let mut feed = Feed {
        updated_at: value.get("updatedAt").and_then(Value::as_str).map(str::to_string),
        packages: Vec::new(),
        skipped: Vec::new(),
        signed: false,
    };
    for item in list {
        match entry(item, base) {
            Ok(entry) if feed.packages.iter().any(|known| known.id == entry.id && known.version == entry.version) => {
                feed.skipped.push(format!("{} {}: дважды", entry.id, entry.version));
            }
            Ok(entry) => feed.packages.push(entry),
            Err(problem) => feed.skipped.push(problem),
        }
    }
    Ok(feed)
}

/// What the cache remembers of the last read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fetched {
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    /// RFC 3339.
    fetched_at: String,
}

/// A preview's kind by its first bytes: a picture (WebP, PNG — never SVG) or Opus in Ogg.
fn preview_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"OggS") {
        Some("ogg")
    } else {
        None
    }
}

pub fn content_type(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "webp" => "image/webp",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// How a package of the feed stands here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryState {
    Available,
    Installed,
    Update,
    Incompatible,
    /// A plugin or an app from a catalog without a signature: listed, never installed (stage 8).
    NeedsSignature,
}

pub const NEEDS_SIGNATURE: &str = "Плагины и приложения ставятся из каталога только с его подписью: у этой сборки нет \
ключа каталога (PACKAGES_PUBKEY) или подпись ленты не сошлась.";

/// Whether a package of the feed can be installed here, and how it stands against the installed version. Code — a
/// plugin, an app — only from a signed feed; on the station, only what fits the plugins' free memory.
pub fn entry_state(
    entry: &Entry,
    installed: Option<&semver::Version>,
    placement: Placement,
    signed: bool,
    free_mb: Option<u64>,
) -> (EntryState, Option<String>) {
    if matches!(entry.kind, PackageKind::Plugin | PackageKind::App) && !signed {
        // What is installed stays installed; a new version of code waits for a signed feed.
        return match installed {
            Some(installed) if *installed >= entry.version => (EntryState::Installed, None),
            _ => (EntryState::NeedsSignature, Some(NEEDS_SIGNATURE.into())),
        };
    }
    if entry.kind == PackageKind::App
        && !entry.platforms.is_empty()
        && !entry.platforms.contains_key(&super::manifest::platform())
    {
        return (
            EntryState::Incompatible,
            Some(format!("нет сборки под эту платформу ({})", super::manifest::platform())),
        );
    }
    if placement.on_station() && matches!(entry.kind, PackageKind::Plugin | PackageKind::App) {
        if let (Some(need), Some(free)) = (entry.memory_mb, free_mb) {
            if need > free && installed.is_none() {
                return (
                    EntryState::Incompatible,
                    Some(format!("на колонке не хватит памяти: нужно {need} МБ, свободно {free}")),
                );
            }
        }
    }
    let gateway = semver::Version::parse(crate::VERSION).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    let why = if !semver::VersionReq::parse(&entry.api).is_ok_and(|requirement| requirement.matches(&HOST_API)) {
        Some(format!("нужен Host API {}, а у шлюза {HOST_API}", entry.api))
    } else if !entry.runs.contains(&placement) {
        Some(format!("не работает здесь: размещение «{}»", placement.title()))
    } else {
        entry
            .min_gateway
            .as_ref()
            .filter(|min| **min > gateway)
            .map(|min| format!("нужен шлюз {min} или новее, а этот — {gateway}"))
    };
    if let Some(why) = why {
        return (EntryState::Incompatible, Some(why));
    }
    match installed {
        Some(installed) if *installed < entry.version => (EntryState::Update, None),
        Some(_) => (EntryState::Installed, None),
        None => (EntryState::Available, None),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreviewView {
    /// `/api/v1/packages/catalog/{id}/preview/image`, when the picture is here.
    pub image: Option<String>,
    /// `/api/v1/packages/catalog/{id}/preview/{n}`, from 1.
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntryView {
    pub id: String,
    pub kind: PackageKind,
    pub version: String,
    pub title: String,
    pub description: Option<String>,
    pub license: Option<String>,
    pub verified: bool,
    pub runs: Vec<&'static str>,
    pub min_gateway: Option<String>,
    pub needs: Value,
    pub preview: PreviewView,
    /// The version installed here.
    pub installed: Option<String>,
    pub state: EntryState,
    /// Why it cannot be installed here.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    /// The feed is signed by the catalog's key: plugins and apps install from it.
    pub signed: bool,
    /// This gateway has the catalog's key at all.
    pub keyed: bool,
    /// A feed is named (the environment or the build), and not `off`.
    pub configured: bool,
    /// The owner's switch (setting packages_catalog).
    pub enabled: bool,
    pub url: Option<String>,
    pub fetched_at: Option<String>,
    /// The last read did not reach the feed: this is the copy before.
    pub offline: bool,
    pub problem: Option<String>,
    pub packages: Vec<CatalogEntryView>,
}

#[derive(Default)]
struct Status {
    offline: bool,
    problem: Option<String>,
    /// Updates told on the bus already: each version once.
    announced: HashSet<String>,
}

pub struct CatalogDeps {
    pub config: PluginsConfig,
    pub host: Arc<PluginHost>,
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub events: EventBus,
    pub placement: PlacementNow,
    /// The home server, on the station with one: the catalog and its archives through it when the station has no
    /// internet of its own (stage 9, «через Mac»). The station checks the feed's signature and the archives' hashes
    /// itself.
    pub home: Option<Arc<dyn super::controller::HomeLink>>,
}

pub struct PackageCatalog {
    source: FeedSource,
    home: Option<Arc<dyn super::controller::HomeLink>>,
    /// The catalog's signing key: code installs only from a feed it signed.
    key: Option<minisign_verify::PublicKey>,
    dir: PathBuf,
    host: Arc<PluginHost>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    events: EventBus,
    placement: PlacementNow,
    status: Mutex<Status>,
    /// One read of the feed at a time.
    reading: tokio::sync::Mutex<()>,
    /// One install, update or removal at a time.
    installing: tokio::sync::Mutex<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Writes a file whole: beside it first, then renamed over it.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

impl PackageCatalog {
    pub fn new(deps: CatalogDeps) -> Arc<PackageCatalog> {
        let source = FeedSource::of(deps.config.feed_url.as_deref(), BUILT_IN_FEED);
        PackageCatalog::with_source(deps, source)
    }

    /// The same, with the feed named here: tests.
    pub fn with_source(deps: CatalogDeps, source: FeedSource) -> Arc<PackageCatalog> {
        let key = deps.config.catalog_key.as_deref().and_then(|text| match crate::updates::verify::public_key(text) {
            Ok(key) => Some(key),
            Err(error) => {
                tracing::warn!(target: "plugins", "packages catalog: the key does not read ({error}): no code from the catalog");
                None
            }
        });
        Arc::new(PackageCatalog {
            source,
            home: deps.home.clone(),
            key,
            dir: deps.config.catalog_dir.clone(),
            host: deps.host,
            db: deps.db,
            settings: deps.settings,
            events: deps.events,
            placement: deps.placement,
            status: Mutex::new(Status::default()),
            reading: tokio::sync::Mutex::new(()),
            installing: tokio::sync::Mutex::new(()),
        })
    }

    pub fn enabled(&self) -> bool {
        self.settings.get(&PACKAGES_CATALOG, None).unwrap_or(true)
    }

    /// The owner's switch.
    pub fn set_enabled(&self, enabled: bool, actor: Option<Actor>) -> Result<(), ApiError> {
        self.settings.set(&PACKAGES_CATALOG, &enabled, actor)
    }

    /// The feed's address, checked; the reason when it cannot be read.
    fn url(&self) -> Option<Result<url::Url, String>> {
        match &self.source {
            FeedSource::Url(url) => Some(feed_url(url)),
            FeedSource::Off | FeedSource::Missing => None,
        }
    }

    fn fetched(&self) -> Option<Fetched> {
        let text = std::fs::read_to_string(self.dir.join("state.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The last good copy of this feed, read again, its signature checked again.
    fn copy(&self, url: &url::Url) -> Option<Feed> {
        self.fetched().filter(|fetched| fetched.url == url.as_str())?;
        let text = std::fs::read_to_string(self.dir.join("catalog.json")).ok()?;
        let mut feed = parse_feed(&text, url).ok()?;
        feed.signed = self.signed(text.as_bytes());
        Some(feed)
    }

    /// Whether the feed's text is signed by the catalog's key (`catalog.json.minisig` beside the copy).
    fn signed(&self, text: &[u8]) -> bool {
        let Some(key) = &self.key else { return false };
        let Ok(signature) = std::fs::read_to_string(self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"))) else {
            return false;
        };
        crate::updates::verify::verify(key, text, &signature).is_ok()
    }

    /// The signature's address: the feed's path and `.minisig`, the same origin.
    fn signature_url(url: &url::Url) -> url::Url {
        let mut signature = url.clone();
        signature.set_path(&format!("{}{SIGNATURE_SUFFIX}", url.path()));
        signature.set_query(None);
        signature
    }

    /// The feed as it stands: read again when there is no copy, the copy is older than 6 hours or `refresh` says
    /// so — and only with the switch on; the copy otherwise, or when the network fails.
    pub async fn feed(&self, refresh: bool) -> Option<Feed> {
        let url = match self.url()? {
            Ok(url) => url,
            Err(problem) => {
                lock(&self.status).problem = Some(problem);
                return None;
            }
        };
        if self.enabled() {
            let stale = |fetched: Option<Fetched>| match fetched.filter(|fetched| fetched.url == url.as_str()) {
                None => true,
                Some(fetched) => chrono::DateTime::parse_from_rfc3339(&fetched.fetched_at).map_or(true, |at| {
                    chrono::Utc::now().signed_duration_since(at).to_std().is_ok_and(|age| age > FRESH)
                }),
            };
            if refresh || stale(self.fetched()) || !self.dir.join("catalog.json").exists() {
                let _turn = self.reading.lock().await;
                // Another reader may have read it while this one waited.
                if refresh || stale(self.fetched()) || !self.dir.join("catalog.json").exists() {
                    self.read(&url).await;
                }
            }
        }
        self.copy(&url)
    }

    /// Reads the feed: a new copy with its previews, or the copy before and why.
    async fn read(&self, url: &url::Url) {
        let known = self.fetched().filter(|fetched| fetched.url == url.as_str());
        let etag =
            known.as_ref().and_then(|fetched| fetched.etag.clone()).filter(|_| self.dir.join("catalog.json").exists());
        let mut request = crate::utils::http::client().get(url.as_str()).timeout(FEED_TIMEOUT);
        if let Some(etag) = &etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                // No internet of its own: the computer at home may have it.
                if self.through_home(url).await {
                    return;
                }
                tracing::warn!(target: "plugins", "packages catalog: {url}: {error}; the copy before stays");
                let mut status = lock(&self.status);
                status.offline = true;
                status.problem = Some(format!("Каталог недоступен: {error}"));
                return;
            }
        };
        let now = now_iso();
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            let fetched = Fetched { url: url.to_string(), etag, fetched_at: now };
            let _ = write_whole(&self.dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default());
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = None;
            return;
        }
        let failed = |problem: String| {
            tracing::warn!(target: "plugins", "packages catalog: {problem}; the copy before stays");
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = Some(problem);
        };
        if !response.status().is_success() {
            if response.status().is_server_error() && self.through_home(url).await {
                return;
            }
            return failed(format!("Каталог ответил HTTP {}", response.status()));
        }
        let new_etag =
            response.headers().get(reqwest::header::ETAG).and_then(|value| value.to_str().ok()).map(str::to_string);
        let mut response = response;
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    body.extend_from_slice(&chunk);
                    if body.len() as u64 > MAX_FEED {
                        return failed(format!("Каталог больше {} КБ", MAX_FEED / 1024));
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let mut status = lock(&self.status);
                    status.offline = true;
                    status.problem = Some(format!("Каталог недоступен: {error}"));
                    return;
                }
            }
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        let feed = match parse_feed(&text, url) {
            Ok(feed) => feed,
            Err(problem) => return failed(format!("Каталог не разобрался: {problem}")),
        };
        for skipped in &feed.skipped {
            tracing::warn!(target: "plugins", "packages catalog: left out — {skipped}");
        }
        self.previews(&feed).await;
        // Its signature, when the gateway has the catalog's key: code comes only from a feed it signed.
        let signature = match &self.key {
            Some(_) => install::download(Self::signature_url(url).as_str(), MAX_SIGNATURE, FEED_TIMEOUT).await.ok(),
            None => None,
        };
        let fetched = Fetched { url: url.to_string(), etag: new_etag, fetched_at: now };
        let signature_file = self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"));
        let written = write_whole(&self.dir.join("catalog.json"), text.as_bytes())
            .and_then(|()| match &signature {
                Some(signature) => write_whole(&signature_file, signature),
                None => match std::fs::remove_file(&signature_file) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                },
            })
            .and_then(|()| {
                write_whole(&self.dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default())
            });
        if let Err(error) = written {
            return failed(format!("Копия каталога не записалась: {error}"));
        }
        if self.key.is_some() && !self.signed(text.as_bytes()) {
            tracing::warn!(target: "plugins", "packages catalog: its signature is missing or does not match: no code from it");
            lock(&self.status).problem =
                Some("Подпись каталога не сошлась: плагины и приложения из него не ставятся.".into());
            self.announce(&feed);
            return;
        }
        {
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = None;
        }
        tracing::info!(target: "plugins", "packages catalog: {} package(s) from {url}", feed.packages.len());
        self.announce(&feed);
    }

    /// The feed through the home server (`/home/v1/catalog`): its copy of the same feed and its signature, written here
    /// as if read; the signature is checked here with this gateway's own key. Whether it came.
    async fn through_home(&self, url: &url::Url) -> bool {
        let Some((home, key)) = self.home.as_ref().and_then(|home| home.up()) else { return false };
        let answer = crate::utils::http::client()
            .get(format!("{}/home/v1/catalog", home.trim_end_matches('/')))
            .bearer_auth(key)
            .timeout(FEED_TIMEOUT)
            .send()
            .await;
        let Ok(answer) = answer else { return false };
        let Ok(body) = answer.json::<Value>().await else { return false };
        let body = body.get("message").cloned().unwrap_or(body);
        if body.get("url").and_then(Value::as_str) != Some(url.as_str()) {
            return false;
        }
        let Some(text) = body.get("feed").and_then(Value::as_str) else { return false };
        let Ok(feed) = parse_feed(text, url) else { return false };
        let fetched = Fetched { url: url.to_string(), etag: None, fetched_at: now_iso() };
        let signature_file = self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"));
        let written = write_whole(&self.dir.join("catalog.json"), text.as_bytes())
            .and_then(|()| match body.get("signature").and_then(Value::as_str) {
                Some(signature) => write_whole(&signature_file, signature.as_bytes()),
                None => std::fs::remove_file(&signature_file).or(Ok(())),
            })
            .and_then(|()| {
                write_whole(&self.dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default())
            });
        if written.is_err() {
            return false;
        }
        {
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = None;
        }
        tracing::info!(target: "plugins", "packages catalog: {} package(s) through the home server", feed.packages.len());
        self.announce(&feed);
        true
    }

    /// A file of the feed (an archive, a preview): straight, or through the home server when that fails.
    async fn download(&self, url: &url::Url, max: u64, timeout: Duration) -> Result<Vec<u8>, String> {
        match install::download(url.as_str(), max, timeout).await {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                let Some((home, key)) = self.home.as_ref().and_then(|home| home.up()) else { return Err(error) };
                let answer = crate::utils::http::client()
                    .get(format!("{}/home/v1/catalog/file", home.trim_end_matches('/')))
                    .query(&[("url", url.as_str())])
                    .bearer_auth(key)
                    .timeout(timeout)
                    .send()
                    .await
                    .map_err(|_| error.clone())?;
                if !answer.status().is_success() {
                    return Err(format!("{error}; через компьютер — HTTP {}", answer.status()));
                }
                let bytes = answer.bytes().await.map_err(|_| error.clone())?;
                if bytes.len() as u64 > max {
                    return Err(format!("файл больше {} КБ", max / 1024));
                }
                Ok(bytes.to_vec())
            }
        }
    }

    /// For the station's gateway that leans on this computer: the copy of the feed and its signature.
    pub fn export_for_station(&self) -> Option<Value> {
        let url = self.url().and_then(Result::ok)?;
        self.fetched().filter(|fetched| fetched.url == url.as_str())?;
        let feed = std::fs::read_to_string(self.dir.join("catalog.json")).ok()?;
        let signature = std::fs::read_to_string(self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"))).ok();
        Some(serde_json::json!({ "url": url.as_str(), "feed": feed, "signature": signature }))
    }

    /// A file for the station: only from this catalog's own origin.
    pub async fn fetch_for_station(&self, wanted: &str) -> Result<Vec<u8>, ApiError> {
        let base = self.url().and_then(Result::ok).ok_or_else(|| ApiError::conflict("Каталог здесь не настроен."))?;
        let url = same_origin(&base, wanted).ok_or_else(|| ApiError::forbidden("Только файлы каталога."))?;
        install::download(url.as_str(), MAX_ARCHIVE, ARCHIVE_TIMEOUT).await.map_err(ApiError::bad_gateway)
    }

    /// The previews of the feed's packages that are not here yet; the ones of versions it no longer names go.
    async fn previews(&self, feed: &Feed) {
        let mut kept: HashSet<PathBuf> = HashSet::new();
        for entry in feed.newest() {
            let dir = self.dir.join(&entry.id).join(entry.version.to_string());
            kept.insert(dir.clone());
            let wanted =
                entry.image.iter().map(|url| ("image".to_string(), url, MAX_IMAGE)).chain(
                    entry.samples.iter().enumerate().map(|(n, url)| (format!("sample-{}", n + 1), url, MAX_SAMPLE)),
                );
            for (name, url, max) in wanted {
                if self.preview_file(&dir, &name).is_some() {
                    continue;
                }
                match install::download(url.as_str(), max, FEED_TIMEOUT).await {
                    Ok(bytes) => match preview_kind(&bytes) {
                        Some(extension) if (name == "image") == (extension != "ogg") => {
                            if let Err(error) = write_whole(&dir.join(format!("{name}.{extension}")), &bytes) {
                                tracing::warn!(target: "plugins", "packages catalog: {}: {error}", dir.display());
                            }
                        }
                        _ => {
                            tracing::warn!(target: "plugins", "packages catalog: {} {name}: not WebP, PNG or Opus", entry.id)
                        }
                    },
                    Err(error) => tracing::warn!(target: "plugins", "packages catalog: {} {name}: {error}", entry.id),
                }
            }
        }
        // Folders of packages and versions the feed no longer has.
        for package in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if !package.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            for version in std::fs::read_dir(package.path()).into_iter().flatten().flatten() {
                if !kept.contains(&version.path()) {
                    let _ = std::fs::remove_dir_all(version.path());
                }
            }
            let _ = std::fs::remove_dir(package.path());
        }
    }

    /// A preview's file in a version's folder: `image.webp`, `sample-1.ogg`.
    fn preview_file(&self, dir: &Path, name: &str) -> Option<(PathBuf, &'static str)> {
        ["webp", "png", "ogg"].into_iter().find_map(|extension| {
            let file = dir.join(format!("{name}.{extension}"));
            file.is_file().then(|| (file, content_type(extension)))
        })
    }

    /// The installed packages the feed has newer versions of: told on the bus, each version once.
    fn announce(&self, feed: &Feed) {
        let mut fresh = Vec::new();
        for package in self.host.installed_all() {
            let Some(entry) = feed.find(&package.manifest.id, None) else { continue };
            if entry.version > package.manifest.version {
                let update = format!("{} {}", entry.id, entry.version);
                if lock(&self.status).announced.insert(update.clone()) {
                    fresh.push(update);
                }
            }
        }
        if !fresh.is_empty() {
            tracing::info!(target: "plugins", "packages catalog: newer versions — {}", fresh.join(", "));
            self.events.emit(GatewayEvent::PackagesCatalog { updates: fresh });
        }
    }

    /// The catalog for the panel: every package of the feed (its newest version) and how it stands here.
    pub async fn view(&self, refresh: bool) -> CatalogView {
        self.host.rescan();
        let feed = self.feed(refresh).await;
        let fetched_at = self
            .url()
            .and_then(Result::ok)
            .and_then(|url| self.fetched().filter(|fetched| fetched.url == url.as_str()))
            .map(|fetched| fetched.fetched_at);
        let placement = (self.placement)();
        let signed = feed.as_ref().is_some_and(|feed| feed.signed);
        let free_mb = self.host.free_memory_mb();
        let installed: BTreeMap<String, semver::Version> = self
            .host
            .installed_all()
            .into_iter()
            .map(|package| (package.manifest.id.clone(), package.manifest.version.clone()))
            .collect();
        let packages = feed
            .as_ref()
            .map(|feed| {
                feed.newest()
                    .into_iter()
                    .map(|entry| {
                        let mine = installed.get(&entry.id);
                        let (state, reason) = entry_state(entry, mine, placement, signed, free_mb);
                        let dir = self.dir.join(&entry.id).join(entry.version.to_string());
                        let base = format!("/api/v1/packages/catalog/{}/preview", entry.id);
                        CatalogEntryView {
                            id: entry.id.clone(),
                            kind: entry.kind,
                            version: entry.version.to_string(),
                            title: entry.title.clone(),
                            description: entry.description.clone(),
                            license: entry.license.clone(),
                            verified: entry.verified,
                            runs: entry.runs.iter().map(|runs| runs.as_str()).collect(),
                            min_gateway: entry.min_gateway.as_ref().map(ToString::to_string),
                            needs: entry.needs.clone(),
                            preview: PreviewView {
                                image: self.preview_file(&dir, "image").map(|_| format!("{base}/image")),
                                samples: (1..=entry.samples.len())
                                    .filter(|n| self.preview_file(&dir, &format!("sample-{n}")).is_some())
                                    .map(|n| format!("{base}/{n}"))
                                    .collect(),
                            },
                            installed: mine.map(ToString::to_string),
                            state,
                            reason,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let status = lock(&self.status);
        let problem = match &self.source {
            FeedSource::Off => Some("Каталог выключен в .env (PACKAGES_FEED_URL=off).".into()),
            FeedSource::Missing => {
                Some("Каталог не настроен: в этой сборке нет его адреса (PACKAGES_FEED_URL).".into())
            }
            FeedSource::Url(_) => status.problem.clone(),
        };
        CatalogView {
            signed,
            keyed: self.key.is_some(),
            configured: matches!(self.source, FeedSource::Url(_)),
            enabled: self.enabled(),
            url: match &self.source {
                FeedSource::Url(url) => Some(url.clone()),
                _ => None,
            },
            fetched_at,
            offline: status.offline,
            problem,
            packages,
        }
    }

    /// A preview of a package in the feed: `image` or the n-th phrase (from 1). Only from the copy here.
    pub async fn preview(&self, id: &str, which: &str) -> Result<(PathBuf, &'static str), ApiError> {
        let missing = || ApiError::not_found("Нет такого превью.");
        let url = self.url().and_then(Result::ok).ok_or_else(missing)?;
        let feed = self.copy(&url).ok_or_else(missing)?;
        let entry = feed.find(id, None).ok_or_else(missing)?;
        let name = match which {
            "image" => "image".to_string(),
            n => match n.parse::<usize>() {
                Ok(n) if (1..=MAX_SAMPLES).contains(&n) => format!("sample-{n}"),
                _ => return Err(missing()),
            },
        };
        let dir = self.dir.join(&entry.id).join(entry.version.to_string());
        self.preview_file(&dir, &name).ok_or_else(missing)
    }

    /// The kind of a package as the feed lists it (its newest version), or as installed: who may install or remove
    /// it depends on it.
    pub async fn kind_of(&self, id: &str) -> Option<PackageKind> {
        if let Some(package) = self.host.installed(id) {
            return Some(package.manifest.kind);
        }
        let url = self.url().and_then(Result::ok)?;
        self.copy(&url).and_then(|feed| feed.find(id, None).map(|entry| entry.kind))
    }

    /// The package from the feed: downloaded, checked against its size and hash, installed. `update` takes the newest
    /// version over the installed one and removes the older ones once the new one is in place.
    pub async fn install(
        &self,
        id: &str,
        version: Option<&str>,
        update: bool,
        actor: Option<Actor>,
    ) -> Result<PackageView, ApiError> {
        if !matches!(self.source, FeedSource::Url(_)) {
            return Err(ApiError::conflict("Каталог не настроен — ставить неоткуда."));
        }
        if !self.enabled() {
            return Err(ApiError::conflict("Каталог выключен в настройках: включите его, чтобы ставить пакеты."));
        }
        self.host.rescan();
        let feed = self.feed(false).await.ok_or_else(|| {
            ApiError::unavailable(lock(&self.status).problem.clone().unwrap_or_else(|| "Каталог недоступен.".into()))
        })?;
        let entry = feed.find(id, version).cloned().ok_or_else(|| match version {
            Some(version) => ApiError::not_found(format!("В каталоге нет {id} {version}.")),
            None => ApiError::not_found(format!("В каталоге нет пакета {id}.")),
        })?;
        let installed = self.host.installed(id).map(|package| package.manifest.version.clone());
        if update && installed.is_none() {
            return Err(ApiError::not_found(format!("Пакет {id} не установлен — обновлять нечего.")));
        }
        let (state, reason) =
            entry_state(&entry, installed.as_ref(), (self.placement)(), feed.signed, self.host.free_memory_mb());
        match state {
            EntryState::NeedsSignature | EntryState::Incompatible => {
                return Err(ApiError::conflict(reason.unwrap_or_default()));
            }
            EntryState::Installed if installed.as_ref() == Some(&entry.version) => {
                return Err(ApiError::conflict(format!("{} {} уже установлен.", entry.title, entry.version)));
            }
            EntryState::Installed if update => {
                return Err(ApiError::conflict(format!(
                    "Новее {} в каталоге нет.",
                    installed.unwrap_or(entry.version)
                )));
            }
            _ => {}
        }
        // Room on the disk (on the station): the archive unpacked, roughly twice its size, and the reserve.
        if let Err(why) = self.host.room_for(entry.archive().size.div_ceil(1_048_576) * 2) {
            return Err(ApiError::conflict(format!("Пакет не поставить: {why}.")));
        }
        let _turn = self.installing.lock().await;
        let archive = entry.archive();
        let bytes = self
            .download(&archive.url, MAX_ARCHIVE, ARCHIVE_TIMEOUT)
            .await
            .map_err(|error| ApiError::bad_gateway(format!("Пакет не скачался: {error}")))?;
        let root = self.host.packages_dir().to_path_buf();
        let version = entry.version.to_string();
        let placed = {
            let (root, id, version, sha256) = (root.clone(), entry.id.clone(), version.clone(), archive.sha256.clone());
            let size = archive.size;
            tokio::task::spawn_blocking(move || {
                install::install_archive(&root, &bytes, Expected { id: &id, version: &version, sha256: &sha256, size })
            })
            .await
            .map_err(ApiError::internal)?
        };
        let placed = placed.map_err(|error| ApiError::bad_request(format!("Пакет не поставлен: {error}.")))?;
        if placed.kind != entry.kind {
            let _ = std::fs::remove_dir_all(&placed.dir);
            self.host.rescan();
            return Err(ApiError::bad_request(format!(
                "Пакет не поставлен: в каталоге он {}, а в архиве — {}.",
                entry.kind.as_str(),
                placed.kind.as_str()
            )));
        }
        // An update: the version before stays to fall back on (a rollback, a new one that does not start); older
        // ones leave once the new one is in place.
        if installed.is_some() {
            if let Err(error) = install::keep_versions(&root, id, 2) {
                tracing::warn!(target: "plugins", "{id}: the older versions are not removed: {error}");
            }
        }
        // Vouched for: the feed says so, and code only when the feed is signed.
        let vouched = entry.verified && (feed.signed || !matches!(entry.kind, PackageKind::Plugin | PackageKind::App));
        registry::set_installed(
            &self.db,
            id,
            registry::SOURCE_CATALOG,
            vouched,
            actor.as_ref().map(|actor| actor.name.as_str()),
        )?;
        // A plugin that is on takes the new version: it starts beside the old one, which stays if it does not.
        if installed.is_some() && self.host.is_active(id) {
            if let Err(error) = self.host.reload(id, actor.clone()).await {
                tracing::warn!(target: "plugins", "{id}: the update does not start, the version before runs: {}", error.message);
            }
        }
        self.host.rescan();
        let change = if installed.is_some() { "updated" } else { "installed" };
        tracing::info!(target: "plugins", "{id} {version}: {change} from the catalog");
        self.host.announce(id, change, Some(version), actor);
        self.host.view_one(id).await
    }

    /// A package removed, every version of it and its row; a plugin's code stops first.
    pub async fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let _turn = self.installing.lock().await;
        self.host.rescan();
        let origin = self.host.origin_of(id);
        if !origin.removable() {
            return Err(ApiError::conflict(format!(
                "«{id}» — {} пакет: он обновляется вместе со шлюзом и не удаляется.{}",
                origin.title(),
                if origin.can_disable() {
                    " Его можно выключить или сделать свою копию."
                } else {
                    ""
                }
            )));
        }
        let root = self.host.packages_dir().to_path_buf();
        if !valid_id(id) || !root.join(id).is_dir() {
            return Err(ApiError::not_found(format!("Нет пакета {id}.")));
        }
        self.host.forget(id).await?;
        install::remove_package(&root, id).map_err(ApiError::internal)?;
        self.host.rescan();
        tracing::info!(target: "plugins", "{id}: removed");
        self.host.announce(id, "removed", None, actor);
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::plugins::install::tests::{character_manifest, vgp};
    use crate::plugins::quickjs::QuickJs;
    use crate::plugins::PluginsDeps;
    use crate::skills::media::sources::MediaSources;
    use axum::extract::State;
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use serde_json::json;
    use sha2::Digest;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const WEBP: &[u8] = b"RIFF\x10\0\0\0WEBPVP8 \0\0\0\0";
    const OPUS: &[u8] = b"OggS\0\x02\0\0\0\0\0\0\0\0";

    /// The site: the feed and its files, the feed's text changeable and its reads counted.
    #[derive(Clone)]
    pub struct Site {
        pub feed: Arc<Mutex<String>>,
        pub files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        pub reads: Arc<AtomicUsize>,
        pub url: String,
        pub down: Arc<std::sync::atomic::AtomicBool>,
    }

    async fn feed(State(site): State<Site>, headers: HeaderMap) -> axum::response::Response {
        if site.down.load(Ordering::SeqCst) {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        site.reads.fetch_add(1, Ordering::SeqCst);
        let text = lock(&site.feed).clone();
        let etag = format!("\"{}\"", hex::encode(&sha2::Sha256::digest(text.as_bytes())[..8]));
        if headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok()) == Some(etag.as_str()) {
            return StatusCode::NOT_MODIFIED.into_response();
        }
        ([(header::ETAG, etag)], text).into_response()
    }

    async fn file(
        State(site): State<Site>,
        axum::extract::Path(path): axum::extract::Path<String>,
    ) -> axum::response::Response {
        match lock(&site.files).get(&path) {
            Some(bytes) => bytes.clone().into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    pub async fn site() -> Site {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/catalog.json", listener.local_addr().unwrap().port());
        let site = Site {
            feed: Arc::new(Mutex::new(String::new())),
            files: Arc::new(Mutex::new(BTreeMap::new())),
            reads: Arc::new(AtomicUsize::new(0)),
            url,
            down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let router =
            Router::new().route("/catalog.json", get(feed)).route("/{*path}", get(file)).with_state(site.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        site
    }

    impl Site {
        /// A package on the site and in its feed.
        pub fn publish(&self, packages: &[(Value, Vec<u8>)]) {
            let mut entries = Vec::new();
            for (entry, archive) in packages {
                let (id, version) = (entry["id"].as_str().unwrap(), entry["version"].as_str().unwrap());
                let path = format!("packages/{id}-{version}.vgp");
                lock(&self.files).insert(path.clone(), archive.clone());
                let mut entry = entry.clone();
                entry["url"] = json!(path);
                entry["sha256"] = json!(hex::encode(sha2::Sha256::digest(archive)));
                entry["size"] = json!(archive.len());
                entries.push(entry);
            }
            lock(&self.files).insert("previews/arthur.webp".into(), WEBP.to_vec());
            lock(&self.files).insert("previews/arthur-1.opus".into(), OPUS.to_vec());
            *lock(&self.feed) =
                json!({ "format": 1, "updatedAt": "2026-11-07T12:00:00Z", "packages": entries }).to_string();
        }
    }

    pub fn arthur(version: &str) -> Value {
        json!({
            "id": "arthur", "kind": "character", "version": version, "title": "Артур",
            "description": "ИИ-дворецкий", "api": "^1", "runs": ["pc", "station", "stationServer"],
            "minGateway": "0.0.1", "license": "CC BY-NC 4.0, некоммерчески", "verified": true,
            "preview": { "image": "previews/arthur.webp", "samples": ["previews/arthur-1.opus"] },
            "needs": [{ "kind": "engine", "id": "omnivoice", "optional": true }]
        })
    }

    pub struct Fixture {
        pub catalog: Arc<PackageCatalog>,
        pub host: Arc<PluginHost>,
        pub events: EventBus,
        pub settings: Arc<SettingsService>,
        pub root: PathBuf,
        _dir: tempfile::TempDir,
    }

    pub fn fixture(source: FeedSource, placement: Placement) -> Fixture {
        fixture_keyed(source, placement, None)
    }

    /// The same with the catalog's key: code installs from a feed it signed.
    pub fn fixture_keyed(source: FeedSource, placement: Placement, key: Option<String>) -> Fixture {
        fixture_with(source, placement, key, None)
    }

    /// The same with a home server: the station's way to the catalog without its own internet.
    pub fn fixture_with(
        source: FeedSource,
        placement: Placement,
        key: Option<String>,
        home: Option<Arc<dyn crate::plugins::controller::HomeLink>>,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let config = PluginsConfig {
            packages_dir: dir.path().join("packages"),
            disabled: false,
            idle_unload_s: 600,
            feed_url: None,
            catalog_dir: dir.path().join("packages-catalog"),
            dev_dir: dir.path().join("packages-dev"),
            catalog_key: key,
            reserve_mb: 40,
            storage_reserve_mb: 15,
            pressure_action: crate::plugins::budget::PressureAction::Disable,
            bundled: false,
        };
        let host = PluginHost::new(PluginsDeps {
            config: config.clone(),
            db: db.clone(),
            settings: settings.clone(),
            events: events.clone(),
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(move || placement),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
        });
        let catalog = PackageCatalog::with_source(
            CatalogDeps {
                config: config.clone(),
                host: host.clone(),
                db,
                settings: settings.clone(),
                events: events.clone(),
                placement: Arc::new(move || placement),
                home,
            },
            source,
        );
        Fixture { catalog, host, events, settings, root: config.packages_dir, _dir: dir }
    }

    #[test]
    fn reads_the_feed_by_its_schema_and_only_files_of_its_own_origin() {
        let base = url::Url::parse("https://duowave.app/packages/catalog.json").unwrap();
        let mut good = arthur("1.2.0");
        good["url"] = json!("packages/arthur-1.2.0.vgp");
        good["sha256"] = json!("A".repeat(64));
        good["size"] = json!(612_000);
        let mut elsewhere = good.clone();
        elsewhere["id"] = json!("palych");
        elsewhere["url"] = json!("https://evil.example/palych.vgp");
        let mut preview_elsewhere = good.clone();
        preview_elsewhere["id"] = json!("bublik");
        preview_elsewhere["preview"]["image"] = json!("//evil.example/x.webp");
        let mut port = good.clone();
        port["id"] = json!("kovcheg");
        port["url"] = json!("https://duowave.app:8443/packages/kovcheg.vgp");
        let mut big = good.clone();
        big["id"] = json!("big");
        big["size"] = json!(MAX_ARCHIVE + 1);
        let text = json!({"format": 1, "packages": [good, elsewhere, preview_elsewhere, port, big, {"id": "Bad"}]})
            .to_string();
        let feed = parse_feed(&text, &base).unwrap();
        assert_eq!(feed.packages.len(), 1);
        let entry = &feed.packages[0];
        assert_eq!(entry.url.as_str(), "https://duowave.app/packages/packages/arthur-1.2.0.vgp");
        assert_eq!(entry.sha256, "a".repeat(64));
        assert_eq!(entry.image.as_ref().unwrap().as_str(), "https://duowave.app/packages/previews/arthur.webp");
        assert_eq!(feed.skipped.len(), 5, "{:?}", feed.skipped);
        assert!(feed.skipped.iter().any(|why| why.starts_with("palych: url")));
        assert!(feed.skipped.iter().any(|why| why.starts_with("bublik: preview.image")));
        assert!(feed.skipped.iter().any(|why| why.starts_with("kovcheg: url")), "another port is another origin");
        assert!(parse_feed("{\"format\": 2, \"packages\": []}", &base).unwrap_err().contains("format 2"));
        assert!(parse_feed("<html>", &base).unwrap_err().starts_with("не JSON"));
        assert!(feed_url("http://duowave.app/catalog.json").is_err());
        assert!(feed_url("http://127.0.0.1:8080/catalog.json").is_ok() && feed_url("http://localhost/c.json").is_ok());
        assert!(feed_url("https://duowave.app/packages/catalog.json").is_ok());
        assert_eq!(FeedSource::of(Some("off"), Some("https://x")), FeedSource::Off);
        assert_eq!(FeedSource::of(None, None), FeedSource::Missing);
        assert_eq!(FeedSource::of(None, Some("https://x/c.json")), FeedSource::Url("https://x/c.json".into()));
    }

    #[test]
    fn tells_how_a_package_stands_here_and_never_installs_code_from_an_unsigned_catalog() {
        let base = url::Url::parse("https://duowave.app/catalog.json").unwrap();
        let mut value = arthur("1.2.0");
        value["url"] = json!("a.vgp");
        value["sha256"] = json!("a".repeat(64));
        value["size"] = json!(10);
        let character = entry(&value, &base).unwrap();
        let installed = semver::Version::new(1, 1, 0);
        assert_eq!(entry_state(&character, None, Placement::Pc, false, None).0, EntryState::Available);
        assert_eq!(entry_state(&character, Some(&installed), Placement::Pc, false, None).0, EntryState::Update);
        assert_eq!(
            entry_state(&character, Some(&character.version), Placement::Pc, false, None).0,
            EntryState::Installed
        );
        let mut plugin = character.clone();
        plugin.kind = PackageKind::Plugin;
        assert_eq!(
            entry_state(&plugin, None, Placement::Pc, false, None),
            (EntryState::NeedsSignature, Some(NEEDS_SIGNATURE.into()))
        );
        let mut app = character.clone();
        app.kind = PackageKind::App;
        assert_eq!(entry_state(&app, None, Placement::Pc, false, None).0, EntryState::NeedsSignature);
        let mut newer = character.clone();
        newer.min_gateway = Some(semver::Version::new(99, 0, 0));
        let (state, why) = entry_state(&newer, None, Placement::Pc, false, None);
        assert_eq!(state, EntryState::Incompatible);
        assert!(why.unwrap().starts_with("нужен шлюз 99.0.0"));
        let mut pc_only = character.clone();
        pc_only.runs = vec![Placement::Pc];
        assert!(entry_state(&pc_only, None, Placement::Station, false, None).1.unwrap().contains("На колонке"));
        let mut api = character;
        api.api = "^2".into();
        assert!(entry_state(&api, None, Placement::Pc, false, None).1.unwrap().contains("Host API ^2"));
    }

    #[tokio::test]
    async fn keeps_the_last_good_copy_with_its_previews_reads_it_again_only_when_stale_and_shows_it_offline() {
        let site = site().await;
        site.publish(&[(arthur("1.2.0"), vgp(&character_manifest("arthur", "1.2.0")))]);
        let fixture = fixture(FeedSource::Url(site.url.clone()), Placement::Pc);
        let catalog = &fixture.catalog;
        let view = catalog.view(false).await;
        assert!(view.configured && view.enabled && !view.offline, "{view:?}");
        assert_eq!(view.problem, None);
        let entry = &view.packages[0];
        assert_eq!(
            (entry.id.as_str(), entry.state, entry.installed.as_deref()),
            ("arthur", EntryState::Available, None)
        );
        assert_eq!(entry.preview.image.as_deref(), Some("/api/v1/packages/catalog/arthur/preview/image"));
        assert_eq!(entry.preview.samples, ["/api/v1/packages/catalog/arthur/preview/1"]);
        assert_eq!(entry.needs[0]["id"], "omnivoice");
        let (file, kind) = catalog.preview("arthur", "image").await.unwrap();
        assert_eq!((std::fs::read(file).unwrap(), kind), (WEBP.to_vec(), "image/webp"));
        assert_eq!(catalog.preview("arthur", "1").await.unwrap().1, "audio/ogg");
        assert_eq!(catalog.preview("arthur", "2").await.unwrap_err().status, StatusCode::NOT_FOUND);
        assert_eq!(site.reads.load(Ordering::SeqCst), 1);

        // Fresh: no second read; asked to refresh — read with the ETag, unchanged.
        catalog.view(false).await;
        assert_eq!(site.reads.load(Ordering::SeqCst), 1);
        catalog.view(true).await;
        assert_eq!(site.reads.load(Ordering::SeqCst), 2);

        // A feed that does not parse: the copy before, and why.
        *lock(&site.feed) = "{ broken".into();
        let view = catalog.view(true).await;
        assert_eq!(view.packages.len(), 1, "the last good copy");
        assert!(view.problem.as_deref().unwrap().starts_with("Каталог не разобрался"), "{:?}", view.problem);
        assert!(!view.offline);

        // No network: the copy, offline.
        site.down.store(true, Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert_eq!(view.packages.len(), 1);
        assert!(view.problem.is_some());
        // The owner's switch: no reading at all, the copy still shown.
        site.down.store(false, Ordering::SeqCst);
        catalog.set_enabled(false, None).unwrap();
        let before = site.reads.load(Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert!(!view.enabled && view.packages.len() == 1);
        assert_eq!(site.reads.load(Ordering::SeqCst), before);
        let refused = catalog.install("arthur", None, false, None).await.unwrap_err();
        assert_eq!(refused.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn says_offline_when_the_feed_cannot_be_reached() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let fixture = fixture(FeedSource::Url(format!("http://127.0.0.1:{closed}/catalog.json")), Placement::Pc);
        let view = fixture.catalog.view(false).await;
        assert!(view.offline && view.packages.is_empty());
        assert!(view.problem.unwrap().starts_with("Каталог недоступен"));
        let off = self::fixture(FeedSource::Off, Placement::Pc).catalog.view(false).await;
        assert!(!off.configured && off.problem.unwrap().contains("PACKAGES_FEED_URL=off"));
        let http = self::fixture(FeedSource::Url("http://duowave.app/c.json".into()), Placement::Pc);
        assert!(http.catalog.view(false).await.problem.unwrap().contains("только по HTTPS"));
    }

    #[tokio::test]
    async fn installs_updates_and_removes_a_character_and_tells_the_bus() {
        let site = site().await;
        site.publish(&[(arthur("1.2.0"), vgp(&character_manifest("arthur", "1.2.0")))]);
        let fixture = fixture(FeedSource::Url(site.url.clone()), Placement::Pc);
        let (catalog, mut events) = (&fixture.catalog, fixture.events.subscribe());
        let actor = Actor { kind: crate::core::identity::PrincipalKind::User, id: "u1".into(), name: "Аня".into() };
        let view = catalog.install("arthur", None, false, Some(actor.clone())).await.unwrap();
        assert_eq!((view.kind, view.version.as_deref()), (Some(PackageKind::Character), Some("1.2.0")));
        assert_eq!((view.source.as_str(), view.verified), ("catalog", true));
        assert_eq!(view.license.as_deref(), Some("CC BY-NC 4.0, некоммерчески"));
        assert!(fixture.root.join("arthur/1.2.0/manifest.json").is_file());
        assert_eq!(catalog.view(false).await.packages[0].state, EntryState::Installed);
        assert_eq!(catalog.install("arthur", None, false, None).await.unwrap_err().status, StatusCode::CONFLICT);
        assert_eq!(catalog.install("arthur", None, true, None).await.unwrap_err().status, StatusCode::CONFLICT);
        assert!(fixture.host.media_assets("arthur").is_some(), "a character's files are served while installed");

        // A newer version: told once, installed by the button; the one before stays to fall back on.
        site.publish(&[
            (arthur("1.2.0"), vgp(&character_manifest("arthur", "1.2.0"))),
            (arthur("1.3.0"), vgp(&character_manifest("arthur", "1.3.0"))),
        ]);
        let view = catalog.view(true).await;
        assert_eq!(
            (view.packages[0].state, view.packages[0].installed.as_deref()),
            (EntryState::Update, Some("1.2.0"))
        );
        catalog.view(true).await;
        let view = catalog.install("arthur", None, true, Some(actor.clone())).await.unwrap();
        assert_eq!(view.version.as_deref(), Some("1.3.0"));
        assert!(fixture.root.join("arthur/1.2.0").exists(), "the version before stays for a rollback");
        let back = fixture.host.rollback("arthur", None).await.unwrap();
        assert_eq!(back.version.as_deref(), Some("1.2.0"));
        assert!(!fixture.root.join("arthur/1.3.0").exists());
        assert!(fixture.host.rollback("arthur", None).await.is_err(), "nothing before 1.2.0");

        catalog.remove("arthur", Some(actor)).await.unwrap();
        assert!(!fixture.root.join("arthur").exists());
        assert!(fixture.host.installed("arthur").is_none());
        assert_eq!(catalog.remove("arthur", None).await.unwrap_err().status, StatusCode::NOT_FOUND);
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            match event.as_ref() {
                GatewayEvent::PackagesChanged { change, .. } => seen.push(change.clone()),
                GatewayEvent::PackagesCatalog { updates } => seen.push(format!("catalog: {}", updates.join(","))),
                _ => {}
            }
        }
        assert_eq!(seen, ["installed", "catalog: arthur 1.3.0", "updated", "rolledBack", "removed"]);
    }

    #[tokio::test]
    async fn refuses_an_archive_the_site_changed_and_code_from_an_unsigned_catalog() {
        let site = site().await;
        let archive = vgp(&character_manifest("arthur", "1.2.0"));
        let mut plugin = arthur("1.0.0");
        plugin["id"] = json!("radio");
        plugin["kind"] = json!("plugin");
        site.publish(&[(arthur("1.2.0"), archive.clone()), (plugin, b"code".to_vec())]);
        // The archive replaced on the site after the feed was written.
        lock(&site.files)
            .insert("packages/arthur-1.2.0.vgp".into(), vgp(&character_manifest("arthur", "1.2.0")).repeat(2));
        let fixture = fixture(FeedSource::Url(site.url.clone()), Placement::Pc);
        let refused = fixture.catalog.install("arthur", None, false, None).await.unwrap_err();
        assert_eq!(refused.status, StatusCode::BAD_REQUEST);
        assert!(refused.message.contains("размер") || refused.message.contains("хеш"), "{}", refused.message);
        assert!(!fixture.root.join("arthur").exists());
        let unsigned = fixture.catalog.install("radio", None, false, None).await.unwrap_err();
        assert_eq!((unsigned.status, unsigned.message.as_str()), (StatusCode::CONFLICT, NEEDS_SIGNATURE));
        let view = fixture.catalog.view(false).await;
        let radio = view.packages.iter().find(|entry| entry.id == "radio").unwrap();
        assert_eq!(radio.state, EntryState::NeedsSignature);
        assert_eq!(
            fixture.catalog.install("nobody", None, false, None).await.unwrap_err().status,
            StatusCode::NOT_FOUND
        );
    }

    /// A plugin's `.vgp`: its manifest and its code.
    fn plugin_vgp(manifest: &Value, code: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
        for (path, bytes) in
            [("manifest.json", manifest.to_string().into_bytes()), ("backend.js", code.as_bytes().to_vec())]
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, bytes.as_slice()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[tokio::test]
    async fn installs_code_only_from_a_feed_the_catalogs_key_signed() {
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let site = site().await;
        let manifest = json!({
            "schema": 1, "kind": "plugin", "id": "clock", "version": "1.0.0", "title": "Часы", "api": "^1",
            "runs": ["pc", "station", "stationServer"], "permissions": ["voice.intents"],
            "contributes": { "intents": [{ "phrases": ["который час на луне"], "handler": "moon" }] }
        });
        let mut entry = arthur("1.0.0");
        entry["id"] = json!("clock");
        entry["kind"] = json!("plugin");
        entry["budget"] = json!({ "memoryMb": 4 });
        site.publish(&[(entry, plugin_vgp(&manifest, "export function moon() { return 'Полночь'; }"))]);
        let sign = |text: &str| {
            minisign::sign(Some(&keys.pk), &keys.sk, std::io::Cursor::new(text.as_bytes()), None, None)
                .unwrap()
                .to_string()
        };
        let feed = lock(&site.feed).clone();
        lock(&site.files).insert("catalog.json.minisig".into(), sign(&feed).into_bytes());
        let keyed = fixture_keyed(FeedSource::Url(site.url.clone()), Placement::Pc, Some(keys.pk.to_base64()));

        let view = keyed.catalog.view(true).await;
        assert!(view.signed && view.keyed, "{view:?}");
        assert_eq!(view.packages[0].state, EntryState::Available);
        let installed = keyed.catalog.install("clock", None, false, None).await.unwrap();
        assert_eq!((installed.origin, installed.verified), (crate::plugins::registry::Origin::Catalog, true));

        // The feed changed without a new signature: no code from it.
        *lock(&site.feed) = format!("{feed} ");
        let view = keyed.catalog.view(true).await;
        assert!(!view.signed);
        assert_eq!(
            view.problem.as_deref(),
            Some("Подпись каталога не сошлась: плагины и приложения из него не ставятся.")
        );
        assert_eq!(view.packages[0].state, EntryState::Installed, "what is installed stays");
        // Without the key nothing of code installs at all.
        let unkeyed = fixture(FeedSource::Url(site.url.clone()), Placement::Pc);
        let refused = unkeyed.catalog.install("clock", None, false, None).await.unwrap_err();
        assert_eq!(refused.message, NEEDS_SIGNATURE);
    }

    #[test]
    fn shows_a_plugin_that_does_not_fit_the_stations_memory_as_not_going_here() {
        let base = url::Url::parse("https://catalog.example/catalog.json").unwrap();
        let mut entry = arthur("1.0.0");
        entry["kind"] = json!("plugin");
        entry["budget"] = json!({ "memoryMb": 30 });
        entry["url"] = json!("packages/a.vgp");
        entry["sha256"] = json!("0".repeat(64));
        entry["size"] = json!(10);
        let feed = parse_feed(&json!({ "format": 1, "packages": [entry] }).to_string(), &base).unwrap();
        let entry = &feed.packages[0];
        let (state, why) = entry_state(entry, None, Placement::Station, true, Some(20));
        assert_eq!(state, EntryState::Incompatible);
        assert!(why.unwrap().contains("нужно 30 МБ, свободно 20"));
        assert_eq!(entry_state(entry, None, Placement::Pc, true, Some(20)).0, EntryState::Available);
    }

    #[tokio::test]
    async fn reads_the_catalog_and_its_archives_through_the_computer_when_the_station_has_no_internet() {
        struct Computer(String);
        impl crate::plugins::controller::HomeLink for Computer {
            fn up(&self) -> Option<(String, String)> {
                Some((self.0.clone(), "pair-key".into()))
            }
        }
        let site = site().await;
        site.publish(&[(arthur("1.0.0"), vgp(&character_manifest("arthur", "1.0.0")))]);
        let feed = lock(&site.feed).clone();
        let archive = lock(&site.files).get("packages/arthur-1.0.0.vgp").cloned().unwrap();
        // The computer: its copy of the feed, and the archive from the catalog's own origin.
        let catalog_url = site.url.clone();
        let computer = Router::new()
            .route(
                "/home/v1/catalog",
                get(move || {
                    let (url, feed) = (catalog_url.clone(), feed.clone());
                    async move { axum::Json(json!({ "url": url, "feed": feed, "signature": null })) }
                }),
            )
            .route(
                "/home/v1/catalog/file",
                get(move || {
                    let archive = archive.clone();
                    async move { archive }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let home = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, computer).await.unwrap() });
        // The station: the catalog's site is out of its reach.
        let fixture = fixture_with(
            FeedSource::Url(site.url.clone()),
            Placement::StationServer,
            None,
            Some(Arc::new(Computer(home))),
        );
        site.down.store(true, Ordering::SeqCst);
        let view = fixture.catalog.view(true).await;
        assert_eq!(view.packages.len(), 1, "{view:?}");
        assert!(!view.offline);
        // The archive too goes the computer's way; its hash is checked here against the feed.
        *lock(&site.files) = BTreeMap::new();
        let installed = fixture.catalog.install("arthur", None, false, None).await.unwrap();
        assert_eq!(installed.version.as_deref(), Some("1.0.0"));
    }
}
