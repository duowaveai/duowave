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
//!
//! The main feed may name extra feeds (`feeds`, up to 4: docs/plans/catalog-feeds.md) — our store, a partner's. Each is
//! read after it, into its own folder `packages-catalog/feeds/<name by its address>/` with its own ETag and state; a
//! feed the main one no longer names goes with its copy. Their packages come after the main's, in the main's order,
//! and an id an earlier feed has is not taken from a later one. Every file from the package's own feed's origin;
//! `verified` only when the main feed's declaration allows it; never code from them — there is no signature for them
//! yet. Only the main feed names feeds: an extra feed's own `feeds` are not followed.
//!
//! Own sources (docs/plans/custom-feeds.md): feeds the admin adds here by a link, after a warning — read after the
//! main feed's, kept in `packages-catalog/sources.json` (up to 4), never vouched for, never code. One that answers 404
//! or 410 is closed: its copy goes and what was installed from it is hidden.
//!
//! A feed may say a package is `hidden`: it is not listed or installed, and the installed one that came from that very
//! feed is hidden here (registry `hidden`) — not listed, its code off, its characters not offered — until the feed
//! stops saying so.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;

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
/// The extra feeds the main one may name.
const MAX_FEEDS: usize = 4;
/// An extra feed's title for people, at most.
const MAX_FEED_TITLE: usize = 40;
/// The extra feeds' folders, in the catalog's own: `feeds/<name by the address>/`.
const FEEDS_DIR: &str = "feeds";
/// The own sources the admin added here: `[{url, addedAt}]`.
const SOURCES_FILE: &str = "sources.json";
/// Own sources, at most.
pub const MAX_SOURCES: usize = 4;

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
    /// Who puts it out: its own `publisher`; an extra feed's package — the one the main feed names for that feed,
    /// whatever the package says.
    pub publisher: Option<String>,
    /// The extra feed it comes from; None — the main one.
    pub extra: Option<ExtraFeed>,
}

/// An extra feed the main one names (`feeds`): read after it, its packages after the main's.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraFeed {
    pub url: url::Url,
    /// Who puts out its packages: lowercase Latin letters, digits and «-».
    pub publisher: String,
    /// What a person sees it called: its publisher when the main feed gives no title.
    pub title: String,
    /// Whether its packages' `verified` stands: without it they count as not checked by us, whatever it says.
    pub verified: bool,
    /// An own source the admin added here, not one the main feed names.
    pub own: bool,
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
    /// The extra feeds it names: the main feed's are read, an extra one's are not followed.
    pub feeds: Vec<ExtraFeed>,
    /// The packages it says to hide (`hidden: true`): not in `packages`, and an installed one from this feed is hidden.
    pub hidden: std::collections::BTreeSet<String>,
}

/// The catalog's signature: the feed's own address and `.minisig`, from the same origin.
pub const SIGNATURE_SUFFIX: &str = ".minisig";
const MAX_SIGNATURE: u64 = 8 * 1024;

/// The keys the catalog is signed with, built into the program the way the updates' are, apart from them (a leaked
/// catalog key gives no gateway release): minisign's `.pub` files one after another — the release key, then a reserve
/// kept offline for the day the release key is lost or leaks (docs/plans/key-rotation.md, «Ключ каталога»). A feed
/// signed by any of them is signed. PACKAGES_PUBKEY replaces them. Empty: no key — no code from the catalog.
pub const BUILT_IN_KEYS: &str = include_str!("../../packages.pub");

impl Feed {
    /// A catalog of nothing yet: own sources' packages go into it when the main feed has no copy here.
    fn empty() -> Feed {
        Feed {
            updated_at: None,
            packages: Vec::new(),
            skipped: Vec::new(),
            signed: false,
            feeds: Vec::new(),
            hidden: Default::default(),
        }
    }

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

    /// An extra feed's packages after these, marked as its own: its publisher, and `verified` only when its
    /// declaration allows. An id these have already is not taken: what was left out, and whose the id is.
    fn take(&mut self, extra: &ExtraFeed, copy: Feed) -> Vec<String> {
        let mut known: BTreeMap<String, String> = BTreeMap::new();
        for entry in &self.packages {
            let whose = entry.extra.as_ref().map_or_else(|| "the main feed".to_string(), |extra| extra.url.to_string());
            known.entry(entry.id.clone()).or_insert(whose);
        }
        let mut left = Vec::new();
        for mut entry in copy.packages {
            if let Some(whose) = known.get(&entry.id) {
                let refused = format!("{} (in {whose} already)", entry.id);
                if !left.contains(&refused) {
                    left.push(refused);
                }
                continue;
            }
            entry.publisher = Some(extra.publisher.clone());
            entry.verified &= extra.verified;
            entry.extra = Some(extra.clone());
            self.packages.push(entry);
        }
        left
    }
}

/// An extra feed as the main one declares it: HTTPS as the main, its publisher an id, a title for people.
fn extra_feed(value: &Value) -> Result<ExtraFeed, String> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let url = feed_url(text("url").ok_or("feeds: нет url")?).map_err(|problem| format!("feeds: {problem}"))?;
    let publisher = text("publisher")
        .filter(|publisher| valid_id(publisher))
        .ok_or_else(|| format!("feeds: {url}: publisher — строчные латинские буквы, цифры и «-»"))?
        .to_string();
    let title = text("title").map(|title| title.chars().take(MAX_FEED_TITLE).collect());
    let verified = value.get("verified").and_then(Value::as_bool).unwrap_or(false);
    Ok(ExtraFeed { url, title: title.unwrap_or_else(|| publisher.clone()), publisher, verified, own: false })
}

/// An own source as a feed: its host is its title and (in letters an id takes) its publisher; never vouched for.
pub fn own_feed(url: &url::Url) -> ExtraFeed {
    let host = url.host_str().unwrap_or("source").to_ascii_lowercase();
    let letters: String = host.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut publisher: String = letters.trim_matches('-').chars().take(32).collect();
    publisher = publisher.trim_end_matches('-').to_string();
    if !valid_id(&publisher) {
        publisher = "own".into();
    }
    let title = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    ExtraFeed {
        url: url.clone(),
        publisher,
        title: title.chars().take(MAX_FEED_TITLE).collect(),
        verified: false,
        own: true,
    }
}

/// An own source as `sources.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnSource {
    url: String,
    added_at: String,
}

/// An extra feed's folder name by its address: the host to read, and a hash of the whole address — never a version.
fn feed_folder(url: &url::Url) -> String {
    let host: String = url
        .host_str()
        .unwrap_or("feed")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    format!("{host}-{}", hex::encode(&sha2::Sha256::digest(url.as_str().as_bytes())[..4]))
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
    let publisher = text("publisher").filter(|publisher| valid_id(publisher)).map(str::to_string);
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
        publisher,
        extra: None,
    })
}

/// Reads a feed by its schema: the whole of it must be one (format 1, a list of packages); a package in it that is
/// not one is left out with its reason, and so is an extra feed it names (`feeds`) that is not one, or past the 4th.
pub fn parse_feed(text: &str, base: &url::Url) -> Result<Feed, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| format!("не JSON: {error}"))?;
    match value.get("format").and_then(Value::as_u64) {
        Some(FEED_FORMAT) => {}
        Some(other) => return Err(format!("format {other}: этот шлюз читает каталог версии {FEED_FORMAT}")),
        None => return Err("нет format".into()),
    }
    let list = value.get("packages").and_then(Value::as_array).ok_or("нет списка packages")?;
    let mut feed =
        Feed { updated_at: value.get("updatedAt").and_then(Value::as_str).map(str::to_string), ..Feed::empty() };
    for item in list {
        // A package to hide needs its id alone: no archive, no previews.
        if item.get("hidden").and_then(Value::as_bool) == Some(true) {
            match item.get("id").and_then(Value::as_str).map(str::trim).filter(|id| valid_id(id)) {
                Some(id) => {
                    feed.hidden.insert(id.to_string());
                }
                None => feed.skipped.push("hidden: нет id".into()),
            }
            continue;
        }
        match entry(item, base) {
            Ok(entry) if feed.packages.iter().any(|known| known.id == entry.id && known.version == entry.version) => {
                feed.skipped.push(format!("{} {}: дважды", entry.id, entry.version));
            }
            Ok(entry) => feed.packages.push(entry),
            Err(problem) => feed.skipped.push(problem),
        }
    }
    match value.get("feeds") {
        None | Some(Value::Null) => {}
        Some(Value::Array(feeds)) => {
            for item in feeds {
                match extra_feed(item) {
                    Ok(extra) if extra.url == *base || feed.feeds.iter().any(|known| known.url == extra.url) => {
                        feed.skipped.push(format!("feeds: {}: эта лента уже читается", extra.url));
                    }
                    Ok(extra) if feed.feeds.len() == MAX_FEEDS => {
                        feed.skipped.push(format!("feeds: {}: больше {MAX_FEEDS} лент", extra.url));
                    }
                    Ok(extra) => feed.feeds.push(extra),
                    Err(problem) => feed.skipped.push(problem),
                }
            }
        }
        Some(_) => feed.skipped.push("feeds: список лент".into()),
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
    /// An own source that answered 404 or 410: closed, its copy gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    closed: bool,
}

/// What a feed's folder remembers of its last read.
fn fetched_in(dir: &Path) -> Option<Fetched> {
    let text = std::fs::read_to_string(dir.join("state.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// A feed's last good copy as text, when the folder holds this address's.
fn text_in(dir: &Path, url: &url::Url) -> Option<String> {
    fetched_in(dir).filter(|fetched| fetched.url == url.as_str())?;
    std::fs::read_to_string(dir.join("catalog.json")).ok()
}

/// How a feed's address answered.
enum Answer {
    /// The feed's text and its ETag.
    Text(String, Option<String>),
    /// 304: the copy here is the one.
    Same,
    /// No answer at all: the network.
    Unreachable(String),
    /// An answer that broke off midway.
    Broken(String),
    Status(reqwest::StatusCode),
    /// More than a feed may be.
    TooBig,
}

/// Asks a feed's address for it, with the ETag of the copy here.
async fn ask(url: &url::Url, etag: Option<&str>) -> Answer {
    let mut request = crate::utils::http::client().get(url.as_str()).timeout(FEED_TIMEOUT);
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => return Answer::Unreachable(error.to_string()),
    };
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Answer::Same;
    }
    if !response.status().is_success() {
        return Answer::Status(response.status());
    }
    let etag = response.headers().get(reqwest::header::ETAG).and_then(|value| value.to_str().ok()).map(str::to_string);
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() as u64 > MAX_FEED {
                    return Answer::TooBig;
                }
            }
            Ok(None) => break,
            Err(error) => return Answer::Broken(error.to_string()),
        }
    }
    Answer::Text(String::from_utf8_lossy(&body).into_owned(), etag)
}

/// Whether a package's manifest has a character whose voice is of no known origin (`voiceOrigin: unknown`).
fn unknown_voice(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else { return false };
    let Ok(manifest) = serde_json::from_str::<Value>(&text) else { return false };
    manifest
        .pointer("/contributes/characters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|character| character.get("voiceOrigin").and_then(Value::as_str) == Some("unknown"))
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
/// plugin, an app — only from a signed feed, and never from an extra one (no signature for them yet); on the station,
/// only what fits the plugins' free memory.
pub fn entry_state(
    entry: &Entry,
    installed: Option<&semver::Version>,
    placement: Placement,
    signed: bool,
    free_mb: Option<u64>,
) -> (EntryState, Option<String>) {
    if matches!(entry.kind, PackageKind::Plugin | PackageKind::App) && (!signed || entry.extra.is_some()) {
        // What is installed stays installed; a new version of code waits for a signed feed.
        let why = match &entry.extra {
            Some(extra) => {
                format!("Приложения из ленты «{}» не ставятся: подписи для дополнительных лент пока нет.", extra.title)
            }
            None => NEEDS_SIGNATURE.into(),
        };
        return match installed {
            Some(installed) if *installed >= entry.version => (EntryState::Installed, None),
            _ => (EntryState::NeedsSignature, Some(why)),
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
    // The Host API is code's: a character or a theme is data, any version of it reads.
    let coded = matches!(entry.kind, PackageKind::Plugin | PackageKind::App);
    let why = if coded && !semver::VersionReq::parse(&entry.api).is_ok_and(|requirement| requirement.matches(&HOST_API))
    {
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
    /// Who puts it out: an extra feed's declared publisher, else the package's own.
    pub publisher: Option<String>,
    /// The title of the extra feed it comes from; None — the main feed.
    pub source: Option<String>,
    /// It comes from an own source the admin added here.
    pub own: bool,
}

/// An extra feed for the panel: whose it is, what it is called, how its last read went.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedView {
    pub url: String,
    pub publisher: String,
    pub title: String,
    pub fetched_at: Option<String>,
    /// The last read did not reach it: its packages are from the copy before.
    pub offline: bool,
    pub problem: Option<String>,
    /// Its packages taken into the catalog: an id an earlier feed has is not counted.
    pub count: usize,
    /// An own source the admin added here (removable), not one the main feed names.
    pub own: bool,
    /// An own source that answered 404 or 410: its packages are gone from the catalog, the installed ones hidden.
    pub closed: bool,
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
    /// The extra feeds the main one names, in its order, then the own sources added here.
    pub feeds: Vec<FeedView>,
}

#[derive(Default)]
struct Status {
    offline: bool,
    problem: Option<String>,
    /// Updates told on the bus already: each version once.
    announced: HashSet<String>,
    /// The extra feeds' by their address: each one named since the gateway started, read or not.
    extras: BTreeMap<String, FeedStatus>,
}

/// How an extra feed's last read went.
#[derive(Debug, Clone, Default)]
struct FeedStatus {
    offline: bool,
    problem: Option<String>,
}

/// How a read of the main feed went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// A new copy is here.
    New,
    /// The copy before stays: the feed is the same, or it failed.
    Kept,
    /// It came through the home server, and the extra feeds' copies with it.
    Home,
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
    /// The catalog's signing keys: code installs only from a feed one of them signed; none, no code.
    keys: Vec<minisign_verify::PublicKey>,
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
        // A list, one key per line, read whole: a line that does not read leaves no key at all, never a shorter list.
        let keys = match deps.config.catalog_key.as_deref().map(crate::updates::verify::public_keys) {
            None => Vec::new(),
            Some(Ok(keys)) => keys,
            Some(Err(error)) => {
                tracing::warn!(target: "plugins", "packages catalog: its keys do not read ({error}): no code from the catalog");
                Vec::new()
            }
        };
        Arc::new(PackageCatalog {
            source,
            home: deps.home.clone(),
            keys,
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
        fetched_in(&self.dir)
    }

    /// The last good copy of the main feed, read again, its signature checked again.
    fn main_copy(&self, url: &url::Url) -> Option<Feed> {
        let text = text_in(&self.dir, url)?;
        let mut feed = parse_feed(&text, url).ok()?;
        feed.signed = self.signed(text.as_bytes());
        Some(feed)
    }

    /// The extra feeds the main feed's copy names.
    fn declared(&self, url: &url::Url) -> Vec<ExtraFeed> {
        text_in(&self.dir, url).and_then(|text| parse_feed(&text, url).ok()).map(|feed| feed.feeds).unwrap_or_default()
    }

    /// An extra feed's folder: its copy, its state, its previews.
    fn extra_dir(&self, url: &url::Url) -> PathBuf {
        self.dir.join(FEEDS_DIR).join(feed_folder(url))
    }

    /// The catalog as its copies here have it: the main feed's packages, then each extra feed's in the order the main
    /// one names them, then the own sources' — an id an earlier feed has is not taken from a later one. With what was
    /// left out so, by feed. Without a copy of the main feed, the own sources' alone.
    fn merged(&self, url: &url::Url) -> Option<(Feed, Vec<String>)> {
        let own = self.own_feeds(url);
        let mut feed = match self.main_copy(url) {
            Some(feed) => feed,
            None if own.iter().any(|extra| text_in(&self.extra_dir(&extra.url), &extra.url).is_some()) => Feed::empty(),
            None => return None,
        };
        let mut left = Vec::new();
        for extra in feed.feeds.clone().into_iter().chain(own) {
            let Some(text) = text_in(&self.extra_dir(&extra.url), &extra.url) else { continue };
            let Ok(copy) = parse_feed(&text, &extra.url) else { continue };
            let refused = feed.take(&extra, copy);
            left.extend(refused.into_iter().map(|refused| format!("{}: {refused}", extra.url)));
        }
        Some((feed, left))
    }

    /// The own sources as `sources.json` keeps them.
    fn sources(&self) -> Vec<OwnSource> {
        std::fs::read_to_string(self.dir.join(SOURCES_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_sources(&self, sources: &[OwnSource]) -> Result<(), ApiError> {
        let text = serde_json::to_vec_pretty(sources).map_err(ApiError::internal)?;
        write_whole(&self.dir.join(SOURCES_FILE), &text).map_err(ApiError::internal)
    }

    /// The own sources added here, as feeds: one the main feed is, or names already, is read as that one.
    fn own_feeds(&self, main: &url::Url) -> Vec<ExtraFeed> {
        let declared = self.declared(main);
        self.sources()
            .into_iter()
            .filter_map(|source| feed_url(&source.url).ok())
            .filter(|url| url != main && !declared.iter().any(|extra| extra.url == *url))
            .map(|url| own_feed(&url))
            .collect()
    }

    /// Every feed read after the main one: the ones it names, then the own sources.
    fn extras(&self, main: &url::Url) -> Vec<ExtraFeed> {
        let mut extras = self.declared(main);
        extras.extend(self.own_feeds(main));
        extras
    }

    /// The last good copies, read again: the main feed's (its signature checked again) and the extra feeds' after it.
    fn copy(&self, url: &url::Url) -> Option<Feed> {
        self.merged(url).map(|(feed, _)| feed)
    }

    /// Whether the main feed's copy names an extra feed without a copy here that was not read since the gateway
    /// started: one updated over a copy an older gateway wrote reads the catalog now, not in six hours.
    fn untried(&self, url: &url::Url) -> bool {
        let declared = self.extras(url);
        let status = lock(&self.status);
        declared.iter().any(|extra| {
            !status.extras.contains_key(extra.url.as_str())
                && text_in(&self.extra_dir(&extra.url), &extra.url).is_none()
        })
    }

    /// Whether the feed's text is signed by one of the catalog's keys (`catalog.json.minisig` beside the copy): the
    /// reserve's signature holds as well as the release key's, so the catalog moves to it without a gateway release.
    fn signed(&self, text: &[u8]) -> bool {
        if self.keys.is_empty() {
            return false;
        }
        let Ok(signature) = std::fs::read_to_string(self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"))) else {
            return false;
        };
        crate::updates::verify::verify_any(&self.keys, text, &signature).is_ok()
    }

    /// The signature's address: the feed's path and `.minisig`, the same origin.
    fn signature_url(url: &url::Url) -> url::Url {
        let mut signature = url.clone();
        signature.set_path(&format!("{}{SIGNATURE_SUFFIX}", url.path()));
        signature.set_query(None);
        signature
    }

    /// The feed as it stands: read again when there is no copy, the copy is older than 6 hours or `refresh` says
    /// so — and only with the switch on; the copy otherwise, or when the network fails. The extra feeds the main one
    /// names are read after it each time, and their packages come after its own.
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
            let due =
                || refresh || stale(self.fetched()) || !self.dir.join("catalog.json").exists() || self.untried(&url);
            if due() {
                let _turn = self.reading.lock().await;
                // Another reader may have read it while this one waited.
                if due() {
                    let read = self.read(&url).await;
                    self.read_extras(&url, read).await;
                }
            }
        }
        self.copy(&url)
    }

    /// Reads the main feed: a new copy with its previews, or the copy before and why.
    async fn read(&self, url: &url::Url) -> Read {
        let known = self.fetched().filter(|fetched| fetched.url == url.as_str());
        let etag =
            known.as_ref().and_then(|fetched| fetched.etag.clone()).filter(|_| self.dir.join("catalog.json").exists());
        let failed = |problem: String| {
            tracing::warn!(target: "plugins", "packages catalog: {problem}; the copy before stays");
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = Some(problem);
            Read::Kept
        };
        let offline = |error: String| {
            tracing::warn!(target: "plugins", "packages catalog: {url}: {error}; the copy before stays");
            let mut status = lock(&self.status);
            status.offline = true;
            status.problem = Some(format!("Каталог недоступен: {error}"));
            Read::Kept
        };
        let (text, new_etag) = match ask(url, etag.as_deref()).await {
            Answer::Text(text, new_etag) => (text, new_etag),
            Answer::Same => {
                let fetched = Fetched { url: url.to_string(), etag, fetched_at: now_iso(), closed: false };
                let _ = write_whole(&self.dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default());
                let mut status = lock(&self.status);
                status.offline = false;
                status.problem = None;
                return Read::Kept;
            }
            // No internet of its own: the computer at home may have it.
            Answer::Unreachable(error) => {
                return if self.through_home(url).await { Read::Home } else { offline(error) };
            }
            Answer::Broken(error) => return offline(error),
            Answer::Status(code) => {
                if code.is_server_error() && self.through_home(url).await {
                    return Read::Home;
                }
                return failed(format!("Каталог ответил HTTP {code}"));
            }
            Answer::TooBig => return failed(format!("Каталог больше {} КБ", MAX_FEED / 1024)),
        };
        let now = now_iso();
        let feed = match parse_feed(&text, url) {
            Ok(feed) => feed,
            Err(problem) => return failed(format!("Каталог не разобрался: {problem}")),
        };
        for skipped in &feed.skipped {
            tracing::warn!(target: "plugins", "packages catalog: left out — {skipped}");
        }
        self.previews(&self.dir, &feed).await;
        // Its signature, when the gateway has the catalog's keys: code comes only from a feed one of them signed.
        let signature = if self.keys.is_empty() {
            None
        } else {
            install::download(Self::signature_url(url).as_str(), MAX_SIGNATURE, FEED_TIMEOUT).await.ok()
        };
        let fetched = Fetched { url: url.to_string(), etag: new_etag, fetched_at: now, closed: false };
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
        if !self.keys.is_empty() && !self.signed(text.as_bytes()) {
            tracing::warn!(target: "plugins", "packages catalog: its signature is missing or does not match: no code from it");
            lock(&self.status).problem = Some("Подпись каталога не сошлась: приложения из него не ставятся.".into());
            return Read::New;
        }
        {
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = None;
        }
        tracing::info!(target: "plugins", "packages catalog: {} package(s) from {url}", feed.packages.len());
        Read::New
    }

    /// After a read of the main feed: each extra feed its copy names is read in turn — unless they came through the
    /// home server with it — and the folders of the ones it no longer names go. When anything new came, the ids a
    /// later feed could not take are logged and the installed packages with newer versions told.
    async fn read_extras(&self, url: &url::Url, main: Read) {
        let declared = self.declared(url);
        let own = self.own_feeds(url);
        self.forget_extras(&declared.iter().chain(&own).cloned().collect::<Vec<_>>());
        let mut new = main != Read::Kept;
        if main != Read::Home {
            for extra in &declared {
                new |= self.read_extra(extra).await;
            }
        }
        // The own sources: read here even when the main feed came through the home server — it does not bring them.
        for extra in &own {
            new |= self.read_extra(extra).await;
        }
        {
            // Each one named counts as tried now, the ones a home server did not bring too.
            let mut status = lock(&self.status);
            for extra in declared.iter().chain(&own) {
                status.extras.entry(extra.url.to_string()).or_default();
            }
        }
        if !new {
            return;
        }
        self.apply_hidden(url);
        let Some((feed, left)) = self.merged(url) else { return };
        for refused in left {
            tracing::warn!(target: "plugins", "packages catalog: not taken from {refused}");
        }
        self.announce(&feed);
    }

    /// The installed packages each feed hides, by the copies here: one installed from a feed is hidden while that feed
    /// says `hidden` for it, or is an own source that closed; shown again when it stops. A package from a feed with no
    /// word here (no copy, not named any more) stays as it was. One from the catalog before 0.6.0 (no feed noted) is
    /// the main feed's.
    fn apply_hidden(&self, main: &url::Url) {
        // By feed address: the ids it hides, or None — it closed and hides all.
        let mut hides: BTreeMap<String, Option<std::collections::BTreeSet<String>>> = BTreeMap::new();
        if let Some(copy) = self.main_copy(main) {
            hides.insert(main.to_string(), Some(copy.hidden));
        }
        for extra in self.extras(main) {
            let dir = self.extra_dir(&extra.url);
            let closed = fetched_in(&dir).is_some_and(|fetched| fetched.closed && fetched.url == extra.url.as_str());
            if extra.own && closed {
                hides.insert(extra.url.to_string(), None);
            } else if let Some(copy) = text_in(&dir, &extra.url).and_then(|text| parse_feed(&text, &extra.url).ok()) {
                hides.insert(extra.url.to_string(), Some(copy.hidden));
            }
        }
        let mut changed = Vec::new();
        for (id, choice) in registry::choices(&self.db) {
            if choice.source != registry::SOURCE_CATALOG {
                continue;
            }
            let feed = choice.feed.clone().unwrap_or_else(|| main.to_string());
            let Some(hides) = hides.get(&feed) else { continue };
            let hidden = hides.as_ref().is_none_or(|ids| ids.contains(&id));
            match registry::set_hidden(&self.db, &id, hidden) {
                Ok(true) => changed.push((id, hidden)),
                Ok(false) => {}
                Err(error) => tracing::warn!(target: "plugins", "{id}: hidden is not noted: {error}"),
            }
        }
        if changed.is_empty() {
            return;
        }
        self.host.rescan();
        for (id, hidden) in changed {
            let change = if hidden { "hidden" } else { "shown" };
            tracing::info!(target: "plugins", "{id}: {change} by its feed");
            self.host.announce(&id, change, None, None);
        }
    }

    /// Reads an extra feed into its folder: a new copy with its previews, or the copy before and why. Whether a new
    /// copy came.
    async fn read_extra(&self, extra: &ExtraFeed) -> bool {
        let (url, title) = (&extra.url, &extra.title);
        let dir = self.extra_dir(url);
        let known = fetched_in(&dir).filter(|fetched| fetched.url == url.as_str());
        let etag = known.and_then(|fetched| fetched.etag).filter(|_| dir.join("catalog.json").exists());
        // A new copy, or none: (offline, why).
        let outcome = match ask(url, etag.as_deref()).await {
            Answer::Same => {
                let fetched = Fetched { url: url.to_string(), etag, fetched_at: now_iso(), closed: false };
                let _ = write_whole(&dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default());
                Ok(false)
            }
            Answer::Unreachable(error) | Answer::Broken(error) => {
                Err((true, format!("Лента «{title}» недоступна: {error}")))
            }
            // An own source taken down: closed — its copy goes, and what came from it is hidden.
            Answer::Status(code)
                if extra.own && matches!(code, reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE) =>
            {
                let was = fetched_in(&dir).is_some_and(|fetched| fetched.closed && fetched.url == url.as_str());
                let _ = std::fs::remove_dir_all(&dir);
                let fetched = Fetched { url: url.to_string(), etag: None, fetched_at: now_iso(), closed: true };
                let _ = write_whole(&dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default());
                if !was {
                    tracing::info!(target: "plugins", "packages catalog: {url}: HTTP {code} — the own source is closed");
                }
                let mut status = lock(&self.status);
                status.extras.insert(
                    url.to_string(),
                    FeedStatus {
                        offline: false, problem: Some(format!("Источник «{title}» закрыт (HTTP {code})."))
                    },
                );
                return !was;
            }
            Answer::Status(code) => Err((false, format!("Лента «{title}» ответила HTTP {code}"))),
            Answer::TooBig => Err((false, format!("Лента «{title}» больше {} КБ", MAX_FEED / 1024))),
            Answer::Text(text, new_etag) => match parse_feed(&text, url) {
                Err(problem) => Err((false, format!("Лента «{title}» не разобралась: {problem}"))),
                Ok(feed) => {
                    for skipped in &feed.skipped {
                        tracing::warn!(target: "plugins", "packages catalog: {url}: left out — {skipped}");
                    }
                    if !feed.feeds.is_empty() {
                        tracing::info!(target: "plugins", "packages catalog: {url}: names feeds of its own — only the main feed's are read");
                    }
                    self.previews(&dir, &feed).await;
                    let fetched =
                        Fetched { url: url.to_string(), etag: new_etag, fetched_at: now_iso(), closed: false };
                    let written = write_whole(&dir.join("catalog.json"), text.as_bytes()).and_then(|()| {
                        write_whole(&dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default())
                    });
                    match written {
                        Ok(()) => {
                            tracing::info!(target: "plugins", "packages catalog: {} package(s) from {url} ({})", feed.packages.len(), extra.publisher);
                            Ok(true)
                        }
                        Err(error) => Err((false, format!("Копия ленты «{title}» не записалась: {error}"))),
                    }
                }
            },
        };
        let mut status = lock(&self.status);
        let state = status.extras.entry(url.to_string()).or_default();
        match outcome {
            Ok(new) => {
                *state = FeedStatus::default();
                new
            }
            Err((offline, problem)) => {
                tracing::warn!(target: "plugins", "packages catalog: {url}: {problem}; the copy before stays");
                *state = FeedStatus { offline, problem: Some(problem) };
                false
            }
        }
    }

    /// The extra feeds the main one no longer names (and the own sources no longer added): their folders go, and
    /// what was known of them.
    fn forget_extras(&self, declared: &[ExtraFeed]) {
        let named: HashSet<String> = declared.iter().map(|extra| feed_folder(&extra.url)).collect();
        for folder in std::fs::read_dir(self.dir.join(FEEDS_DIR)).into_iter().flatten().flatten() {
            let name = folder.file_name().to_string_lossy().into_owned();
            // A version's folder is the previews of a main feed's package that happens to be called `feeds`.
            if named.contains(&name) || semver::Version::parse(&name).is_ok() {
                continue;
            }
            tracing::info!(target: "plugins", "packages catalog: the feed {name} is no longer named — its copy goes");
            let _ = std::fs::remove_dir_all(folder.path());
        }
        lock(&self.status).extras.retain(|url, _| declared.iter().any(|extra| extra.url.as_str() == url));
    }

    /// The feed through the home server (`/home/v1/catalog`): its copy of the same feed and its signature, written here
    /// as if read; the signature is checked here with this gateway's own key. The extra feeds' copies come with it
    /// (`feeds`: none from a computer older than them — the main feed alone then). Whether it came.
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
        let fetched = Fetched { url: url.to_string(), etag: None, fetched_at: now_iso(), closed: false };
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
        // The extra feeds' copies, of the feeds this main one names.
        for copy in body.get("feeds").and_then(Value::as_array).into_iter().flatten() {
            let address = copy.get("url").and_then(Value::as_str);
            let Some(extra) = feed.feeds.iter().find(|extra| Some(extra.url.as_str()) == address) else { continue };
            let Some(text) = copy.get("feed").and_then(Value::as_str) else { continue };
            if parse_feed(text, &extra.url).is_err() {
                continue;
            }
            let dir = self.extra_dir(&extra.url);
            let fetched = Fetched { url: extra.url.to_string(), etag: None, fetched_at: now_iso(), closed: false };
            let written = write_whole(&dir.join("catalog.json"), text.as_bytes())
                .and_then(|()| write_whole(&dir.join("state.json"), &serde_json::to_vec(&fetched).unwrap_or_default()));
            if written.is_ok() {
                lock(&self.status).extras.insert(extra.url.to_string(), FeedStatus::default());
            }
        }
        {
            let mut status = lock(&self.status);
            status.offline = false;
            status.problem = None;
        }
        tracing::info!(target: "plugins", "packages catalog: {} package(s) through the home server", feed.packages.len());
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

    /// For the station's gateway that leans on this computer: the copy of the feed and its signature, and the copies
    /// of the extra feeds it names (`feeds: [{url, feed}]`).
    pub fn export_for_station(&self) -> Option<Value> {
        let url = self.url().and_then(Result::ok)?;
        let feed = text_in(&self.dir, &url)?;
        let signature = std::fs::read_to_string(self.dir.join(format!("catalog.json{SIGNATURE_SUFFIX}"))).ok();
        let feeds: Vec<Value> = self
            .declared(&url)
            .into_iter()
            .filter_map(|extra| {
                let copy = text_in(&self.extra_dir(&extra.url), &extra.url)?;
                Some(serde_json::json!({ "url": extra.url.as_str(), "feed": copy }))
            })
            .collect();
        Some(serde_json::json!({ "url": url.as_str(), "feed": feed, "signature": signature, "feeds": feeds }))
    }

    /// A file for the station: only from the origin of a feed this computer reads — the main one, or an extra one it
    /// names.
    pub async fn fetch_for_station(&self, wanted: &str) -> Result<Vec<u8>, ApiError> {
        let base = self.url().and_then(Result::ok).ok_or_else(|| ApiError::conflict("Каталог здесь не настроен."))?;
        let extras = self.declared(&base).into_iter().map(|extra| extra.url);
        let url = std::iter::once(base).chain(extras).find_map(|base| same_origin(&base, wanted));
        let url = url.ok_or_else(|| ApiError::forbidden("Только файлы каталога."))?;
        install::download(url.as_str(), MAX_ARCHIVE, ARCHIVE_TIMEOUT).await.map_err(ApiError::bad_gateway)
    }

    /// The previews of a feed's packages that are not here yet, in its folder; the ones of versions it no longer
    /// names go.
    async fn previews(&self, root: &Path, feed: &Feed) {
        let mut kept: HashSet<PathBuf> = HashSet::new();
        for entry in feed.newest() {
            let dir = root.join(&entry.id).join(entry.version.to_string());
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
        for package in std::fs::read_dir(root).into_iter().flatten().flatten() {
            if !package.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            for version in std::fs::read_dir(package.path()).into_iter().flatten().flatten() {
                // The extra feeds' folders are theirs (in `feeds`, beside a version of a package of that name).
                let extra = package.file_name() == FEEDS_DIR
                    && semver::Version::parse(&version.file_name().to_string_lossy()).is_err();
                if !extra && !kept.contains(&version.path()) {
                    let _ = std::fs::remove_dir_all(version.path());
                }
            }
            let _ = std::fs::remove_dir(package.path());
        }
    }

    /// A package's previews: in its feed's folder, by its id and version.
    fn preview_dir(&self, entry: &Entry) -> PathBuf {
        let feed = match &entry.extra {
            Some(extra) => self.extra_dir(&extra.url),
            None => self.dir.clone(),
        };
        feed.join(&entry.id).join(entry.version.to_string())
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

    /// The catalog for the panel: every package of the feeds (its newest version) and how it stands here, and how each
    /// extra feed stands.
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
                        let dir = self.preview_dir(entry);
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
                            publisher: entry.publisher.clone(),
                            source: entry.extra.as_ref().map(|extra| extra.title.clone()),
                            own: entry.extra.as_ref().is_some_and(|extra| extra.own),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The own sources are listed even without a copy: the admin removes them here.
        let own = self.url().and_then(Result::ok).map(|url| self.own_feeds(&url)).unwrap_or_default();
        let status = lock(&self.status);
        let newest = feed.as_ref().map(|feed| feed.newest()).unwrap_or_default();
        let feeds = feed
            .as_ref()
            .map(|feed| feed.feeds.clone())
            .unwrap_or_default()
            .into_iter()
            .chain(own)
            .map(|extra| {
                let known = status.extras.get(extra.url.as_str());
                let fetched =
                    fetched_in(&self.extra_dir(&extra.url)).filter(|fetched| fetched.url == extra.url.as_str());
                FeedView {
                    url: extra.url.to_string(),
                    publisher: extra.publisher.clone(),
                    title: extra.title.clone(),
                    fetched_at: fetched.as_ref().map(|fetched| fetched.fetched_at.clone()),
                    offline: known.is_some_and(|known| known.offline),
                    problem: known.and_then(|known| known.problem.clone()),
                    count: newest
                        .iter()
                        .filter(|entry| entry.extra.as_ref().is_some_and(|from| from.url == extra.url))
                        .count(),
                    own: extra.own,
                    closed: extra.own && fetched.is_some_and(|fetched| fetched.closed),
                }
            })
            .collect();
        let problem = match &self.source {
            FeedSource::Off => Some("Каталог выключен в .env (PACKAGES_FEED_URL=off).".into()),
            FeedSource::Missing => {
                Some("Каталог не настроен: в этой сборке нет его адреса (PACKAGES_FEED_URL).".into())
            }
            FeedSource::Url(_) => status.problem.clone(),
        };
        CatalogView {
            signed,
            keyed: !self.keys.is_empty(),
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
            feeds,
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
        self.preview_file(&self.preview_dir(entry), &name).ok_or_else(missing)
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
            return Err(ApiError::conflict("Каталог выключен в настройках: включите его, чтобы ставить приложения."));
        }
        self.host.rescan();
        let feed = self.feed(false).await.ok_or_else(|| {
            ApiError::unavailable(lock(&self.status).problem.clone().unwrap_or_else(|| "Каталог недоступен.".into()))
        })?;
        let entry = feed.find(id, version).cloned().ok_or_else(|| match version {
            Some(version) => ApiError::not_found(format!("В каталоге нет {id} {version}.")),
            None => ApiError::not_found(format!("В каталоге нет «{id}».")),
        })?;
        let installed = self.host.installed(id).map(|package| package.manifest.version.clone());
        if update && installed.is_none() {
            return Err(ApiError::not_found(format!("«{id}» не установлено — обновлять нечего.")));
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
            return Err(ApiError::conflict(format!("Не поставить: {why}.")));
        }
        let _turn = self.installing.lock().await;
        let archive = entry.archive();
        let bytes = self
            .download(&archive.url, MAX_ARCHIVE, ARCHIVE_TIMEOUT)
            .await
            .map_err(|error| ApiError::bad_gateway(format!("Не скачалось: {error}")))?;
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
        let placed = placed.map_err(|error| ApiError::bad_request(format!("Не поставлено: {error}.")))?;
        // A voice of no known origin comes only from an own source the admin added, never from the catalog.
        let own = entry.extra.as_ref().is_some_and(|extra| extra.own);
        if !own && unknown_voice(&placed.dir) {
            let _ = std::fs::remove_dir_all(&placed.dir);
            self.host.rescan();
            return Err(ApiError::bad_request(format!(
                "Не поставлено: у «{}» голос неизвестного происхождения (voiceOrigin unknown) — из каталога такое не \
                 ставится.",
                entry.title
            )));
        }
        if placed.kind != entry.kind {
            let _ = std::fs::remove_dir_all(&placed.dir);
            self.host.rescan();
            return Err(ApiError::bad_request(format!(
                "Не поставлено: в каталоге это {}, а в архиве — {}.",
                entry.kind.as_str(),
                placed.kind.as_str()
            )));
        }
        // The version in the app's history (docs/plans/package-history.md), with the plugin's data just before it.
        if let Some(history) = self.host.history() {
            history.note_install(
                id,
                &placed.dir,
                &version,
                super::history::RecordKind::Catalog,
                super::history::Author {
                    who: super::history::Who::Catalog,
                    name: actor.as_ref().map(|actor| actor.name.clone()),
                    via: None,
                },
                false,
                false,
                None,
            );
        }
        // An update: the version before stays to fall back on (a rollback, a new one that does not start); older
        // ones leave once the new one is in place — they are in the history.
        if installed.is_some() {
            if let Err(error) = install::keep_versions(&root, id, 2) {
                tracing::warn!(target: "plugins", "{id}: the older versions are not removed: {error}");
            }
        }
        // Vouched for: the feed says so (an extra one's only when the main allows), and code only from a signed main.
        let signed = feed.signed && entry.extra.is_none();
        let vouched = entry.verified && (signed || !matches!(entry.kind, PackageKind::Plugin | PackageKind::App));
        registry::set_installed(
            &self.db,
            id,
            registry::SOURCE_CATALOG,
            vouched,
            actor.as_ref().map(|actor| actor.name.as_str()),
        )?;
        // Only the feed it came from may hide it.
        let from = entry.extra.as_ref().map(|extra| extra.url.clone());
        if let Some(from) = from.or_else(|| self.url().and_then(Result::ok)) {
            registry::set_feed(&self.db, id, from.as_str())?;
        }
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

    /// An own source added by its link (docs/plans/custom-feeds.md): it must read as a feed first; then it is kept,
    /// its copy and previews taken, and its packages join the catalog after the others'.
    pub async fn add_source(&self, link: &str, actor: Option<Actor>) -> Result<CatalogView, ApiError> {
        let main = match self.url() {
            Some(Ok(url)) => url,
            _ => return Err(ApiError::conflict("Каталог не настроен: свои источники читаются вместе с ним.")),
        };
        if !self.enabled() {
            return Err(ApiError::conflict("Каталог выключен в настройках: включите его, чтобы добавить источник."));
        }
        let url = feed_url(link.trim()).map_err(ApiError::bad_request)?;
        if url == main || self.declared(&main).iter().any(|extra| extra.url == url) {
            return Err(ApiError::conflict("Эта лента уже читается: её пакеты и так в каталоге."));
        }
        {
            let sources = self.sources();
            if sources.iter().any(|source| source.url == url.as_str()) {
                return Err(ApiError::conflict("Этот источник уже добавлен."));
            }
            if sources.len() >= MAX_SOURCES {
                return Err(ApiError::conflict(format!(
                    "Своих источников — не больше {MAX_SOURCES}: сначала уберите ненужный."
                )));
            }
        }
        // It must answer as a feed: a link with a typo is not kept.
        let problem = match ask(&url, None).await {
            Answer::Text(text, _) => {
                parse_feed(&text, &url).err().map(|problem| format!("это не лента пакетов: {problem}"))
            }
            Answer::Same => None,
            Answer::Unreachable(error) | Answer::Broken(error) => Some(format!("не отвечает: {error}")),
            Answer::Status(code) => Some(format!("ответил HTTP {code}")),
            Answer::TooBig => Some(format!("лента больше {} КБ", MAX_FEED / 1024)),
        };
        if let Some(problem) = problem {
            return Err(ApiError::bad_gateway(format!("Источник не добавлен: {url} {problem}.")));
        }
        {
            let _turn = self.reading.lock().await;
            let mut sources = self.sources();
            if !sources.iter().any(|source| source.url == url.as_str()) {
                sources.push(OwnSource { url: url.to_string(), added_at: now_iso() });
                self.write_sources(&sources)?;
            }
            let who = actor.as_ref().map_or("—", |actor| actor.name.as_str());
            tracing::info!(target: "plugins", "packages catalog: own source added — {url} (by {who})");
            let extra = own_feed(&url);
            let _ = std::fs::remove_dir_all(self.extra_dir(&url));
            self.read_extra(&extra).await;
            self.apply_hidden(&main);
            if let Some((feed, _)) = self.merged(&main) {
                self.announce(&feed);
            }
        }
        Ok(self.view(false).await)
    }

    /// An own source removed: its copy goes, and its packages from the catalog. What was installed from it stays — as
    /// it is, hidden or not.
    pub async fn remove_source(&self, link: &str, actor: Option<Actor>) -> Result<CatalogView, ApiError> {
        let url = feed_url(link.trim()).map_err(ApiError::bad_request)?;
        {
            let _turn = self.reading.lock().await;
            let mut sources = self.sources();
            let before = sources.len();
            sources.retain(|source| feed_url(&source.url).ok().as_ref() != Some(&url));
            if sources.len() == before {
                return Err(ApiError::not_found("Такого источника здесь нет."));
            }
            self.write_sources(&sources)?;
            let _ = std::fs::remove_dir_all(self.extra_dir(&url));
            lock(&self.status).extras.remove(url.as_str());
            let who = actor.as_ref().map_or("—", |actor| actor.name.as_str());
            tracing::info!(target: "plugins", "packages catalog: own source removed — {url} (by {who})");
        }
        Ok(self.view(false).await)
    }

    /// A package removed, every version of it and its row; a plugin's code stops first.
    pub async fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let _turn = self.installing.lock().await;
        self.host.rescan();
        let origin = self.host.origin_of(id);
        if !origin.removable() {
            return Err(ApiError::conflict(format!(
                "«{id}» — {} приложение: оно обновляется вместе со шлюзом и не удаляется.{}",
                origin.word(),
                if origin.can_disable() {
                    " Его можно выключить или сделать свою копию."
                } else {
                    ""
                }
            )));
        }
        let root = self.host.packages_dir().to_path_buf();
        if !valid_id(id) || !root.join(id).is_dir() {
            return Err(ApiError::not_found(format!("Нет приложения {id}.")));
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
    use crate::plugins::controller::PackagesView;
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
        /// Files to refuse, one request each: a station's own try, before the computer's.
        pub refuse: Arc<AtomicUsize>,
        /// Taken down: the feed answers 404.
        pub gone: Arc<std::sync::atomic::AtomicBool>,
    }

    async fn feed(State(site): State<Site>, headers: HeaderMap) -> axum::response::Response {
        if site.down.load(Ordering::SeqCst) {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if site.gone.load(Ordering::SeqCst) {
            return StatusCode::NOT_FOUND.into_response();
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
        if site.refuse.load(Ordering::SeqCst) > 0 {
            site.refuse.fetch_sub(1, Ordering::SeqCst);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
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
            refuse: Arc::new(AtomicUsize::new(0)),
            gone: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

        /// The extra feeds its feed names (`feeds`).
        pub fn name_feeds(&self, feeds: Value) {
            let mut feed: Value = serde_json::from_str(&lock(&self.feed)).unwrap();
            feed["feeds"] = feeds;
            *lock(&self.feed) = feed.to_string();
        }

        /// A package added to its feed as it is: its addresses are not rewritten.
        fn add(&self, entry: Value) {
            let mut feed: Value = serde_json::from_str(&lock(&self.feed)).unwrap();
            feed["packages"].as_array_mut().unwrap().push(entry);
            *lock(&self.feed) = feed.to_string();
        }

        /// Its scheme, host and port.
        fn origin(&self) -> &str {
            self.url.trim_end_matches("/catalog.json")
        }

        fn address(&self) -> url::Url {
            url::Url::parse(&self.url).unwrap()
        }
    }

    /// A character with its archive, as Arthur is.
    fn character(id: &str, version: &str) -> (Value, Vec<u8>) {
        let mut entry = arthur(version);
        entry["id"] = json!(id);
        (entry, vgp(&character_manifest(id, version)))
    }

    /// The ids in the view, in its order.
    fn ids(view: &CatalogView) -> Vec<&str> {
        view.packages.iter().map(|entry| entry.id.as_str()).collect()
    }

    pub fn arthur(version: &str) -> Value {
        json!({
            "id": "arthur", "kind": "character", "version": version, "title": "Артур",
            "description": "ИИ-дворецкий", "api": "^2", "runs": ["pc", "station", "stationServer"],
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
            schedules: true,
        };
        let host = PluginHost::new(PluginsDeps {
            config: config.clone(),
            db: db.clone(),
            settings: settings.clone(),
            events: events.clone(),
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(move || placement),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
            secrets: Arc::new(
                crate::secrets::SecretsService::new(Arc::new(crate::storage::Db::memory()), None).unwrap(),
            ),
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
        // A plugin for another Host API cannot run here; a character is data, any Host API reads.
        let mut api = plugin.clone();
        api.api = "^3".into();
        assert!(entry_state(&api, None, Placement::Pc, true, None).1.unwrap().contains("Host API ^3"));
        let mut old_character = character;
        old_character.api = "^1".into();
        assert_eq!(entry_state(&old_character, None, Placement::Pc, false, None).0, EntryState::Available);
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
            "schema": 1, "kind": "plugin", "id": "clock", "version": "1.0.0", "title": "Часы", "api": "^2",
            "runs": ["pc", "station", "stationServer"], "permissions": [],
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
        assert_eq!(view.problem.as_deref(), Some("Подпись каталога не сошлась: приложения из него не ставятся."));
        assert_eq!(view.packages[0].state, EntryState::Installed, "what is installed stays");
        // Without the key nothing of code installs at all.
        let unkeyed = fixture(FeedSource::Url(site.url.clone()), Placement::Pc);
        let refused = unkeyed.catalog.install("clock", None, false, None).await.unwrap_err();
        assert_eq!(refused.message, NEEDS_SIGNATURE);
    }

    #[test]
    fn the_built_in_keys_are_the_release_one_and_the_reserve() {
        // A change here is a rotation, made on purpose (docs/plans/key-rotation.md, «Ключ каталога пакетов»).
        let keys = crate::updates::verify::public_keys(BUILT_IN_KEYS).expect("packages.pub reads");
        assert_eq!(keys.len(), 2, "the release key and the reserve");
        // Apart from the update keys: a leaked catalog key signs no gateway release, and the other way round.
        use base64::Engine as _;
        let updates: String = crate::updates::verify::BUILT_IN_KEY
            .lines()
            .filter_map(|line| base64::engine::general_purpose::STANDARD.decode(line.trim()).ok())
            .map(|text| String::from_utf8_lossy(&text).into_owned())
            .collect();
        assert!(updates.contains("minisign public key"), "updates.pub decodes: {updates}");
        for key in BUILT_IN_KEYS.lines().filter(|line| !line.is_empty() && !line.starts_with("untrusted comment:")) {
            assert!(!updates.contains(key.trim()), "{key} is an update key too");
        }
    }

    #[tokio::test]
    async fn a_feed_signed_by_any_key_of_the_list_is_signed_and_by_no_other() {
        let release = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let reserve = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let stranger = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let site = site().await;
        site.publish(&[(arthur("1.0.0"), vgp(&character_manifest("arthur", "1.0.0")))]);
        let feed = lock(&site.feed).clone();
        // packages.pub's form: minisign's .pub files one after another, each with its comment line.
        let list = format!("{}\n{}\n", release.pk.to_box().unwrap(), reserve.pk.to_box().unwrap());
        let view_signed_by = |signer: &minisign::KeyPair, keys: &str| {
            let signature = minisign::sign(Some(&signer.pk), &signer.sk, feed.as_bytes(), None, None).unwrap();
            lock(&site.files).insert("catalog.json.minisig".into(), signature.to_string().into_bytes());
            let fixture = fixture_keyed(FeedSource::Url(site.url.clone()), Placement::Pc, Some(keys.to_string()));
            async move { fixture.catalog.view(true).await }
        };

        let by_release = view_signed_by(&release, &list).await;
        assert!(by_release.keyed && by_release.signed, "{by_release:?}");
        let by_reserve = view_signed_by(&reserve, &list).await;
        assert!(by_reserve.signed, "the reserve signs without a gateway release: {by_reserve:?}");
        let by_stranger = view_signed_by(&stranger, &list).await;
        assert!(by_stranger.keyed && !by_stranger.signed, "{by_stranger:?}");
        assert_eq!(
            by_stranger.problem.as_deref(),
            Some("Подпись каталога не сошлась: приложения из него не ставятся.")
        );
        // PACKAGES_PUBKEY's bare lines do as well; a line that does not read leaves no key at all.
        let bare = format!("{}\n{}", release.pk.to_base64(), reserve.pk.to_base64());
        assert!(view_signed_by(&reserve, &bare).await.signed);
        let broken = view_signed_by(&release, &format!("{list}nonsense\n")).await;
        assert!(!broken.keyed && !broken.signed, "{broken:?}");
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

    #[test]
    fn reads_the_extra_feeds_the_main_one_names_by_their_rules() {
        let base = url::Url::parse("https://packages.duowave.app/catalog.json").unwrap();
        let feeds = json!([
            { "url": "https://store.duowave.app/catalog.json", "publisher": "duowave", "title": "DuoWave Store",
              "verified": true },
            { "url": "http://partner.example/catalog.json", "publisher": "partner" },
            { "url": "https://partner.example/catalog.json", "publisher": "Partner" },
            { "url": "https://partner.example/catalog.json", "publisher": "partner", "title": "П".repeat(60) },
            { "url": "https://store.duowave.app/catalog.json", "publisher": "again" },
            { "url": "https://packages.duowave.app/catalog.json", "publisher": "itself" },
            { "url": "https://a.example/catalog.json", "publisher": "a" },
            { "url": "http://127.0.0.1:8080/catalog.json", "publisher": "local" },
            { "url": "https://b.example/catalog.json", "publisher": "b" }
        ]);
        let feed = parse_feed(&json!({ "format": 1, "packages": [], "feeds": feeds }).to_string(), &base).unwrap();
        let urls: Vec<&str> = feed.feeds.iter().map(|extra| extra.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://store.duowave.app/catalog.json",
                "https://partner.example/catalog.json",
                "https://a.example/catalog.json",
                "http://127.0.0.1:8080/catalog.json"
            ]
        );
        let store = &feed.feeds[0];
        assert_eq!(
            (store.publisher.as_str(), store.title.as_str(), store.verified),
            ("duowave", "DuoWave Store", true)
        );
        assert_eq!((feed.feeds[1].title.chars().count(), feed.feeds[1].verified), (MAX_FEED_TITLE, false));
        assert_eq!(feed.feeds[2].title, "a", "without a title — its publisher");
        // HTTP elsewhere, a publisher that is no id, the store twice, the main itself, a fifth feed.
        assert_eq!(feed.skipped.len(), 5, "{:?}", feed.skipped);
        assert!(feed.skipped.iter().any(|why| why.contains("только по HTTPS")));
        assert!(feed.skipped.iter().any(|why| why.contains("больше 4 лент")));
        let odd = parse_feed(&json!({ "format": 1, "packages": [], "feeds": {} }).to_string(), &base).unwrap();
        assert_eq!(odd.skipped, ["feeds: список лент"]);
        let older = parse_feed(&json!({ "format": 1, "packages": [] }).to_string(), &base).unwrap();
        assert!(older.feeds.is_empty() && older.skipped.is_empty());
        // Each address its own folder, never one a package's version could take.
        let folder = feed_folder(&store.url);
        assert!(folder.starts_with("store.duowave.app-") && semver::Version::parse(&folder).is_err());
        let path = |path: &str| url::Url::parse(&format!("https://store.duowave.app/{path}")).unwrap();
        assert_ne!(feed_folder(&path("a/catalog.json")), feed_folder(&path("b/catalog.json")));
        // A package's own publisher, when it is an id.
        let mut value = arthur("1.0.0");
        value["url"] = json!("a.vgp");
        value["sha256"] = json!("a".repeat(64));
        value["size"] = json!(10);
        value["publisher"] = json!("duowave");
        assert_eq!(entry(&value, &base).unwrap().publisher.as_deref(), Some("duowave"));
        value["publisher"] = json!("Duo Wave");
        assert_eq!(entry(&value, &base).unwrap().publisher, None);
    }

    #[tokio::test]
    async fn shows_the_extra_feeds_packages_after_the_main_ones_each_from_its_own_site() {
        let (main, store) = (site().await, site().await);
        let (mut butler, archive) = character("arthur", "1.2.0");
        butler["publisher"] = json!("duowave");
        main.publish(&[(butler, archive)]);
        let (mut bublik, archive) = character("bublik", "1.0.0");
        bublik["publisher"] = json!("impostor");
        store.publish(&[(bublik, archive), character("arthur", "9.0.0")]);
        // A package of the store whose archive is on the main site: only files of its own feed's origin.
        let mut stray = arthur("1.0.0");
        stray["id"] = json!("stray");
        stray["url"] = json!(format!("{}/packages/arthur-1.2.0.vgp", main.origin()));
        stray["sha256"] = json!("a".repeat(64));
        stray["size"] = json!(10);
        store.add(stray);
        let declared = json!({ "url": store.url, "publisher": "duowave", "title": "DuoWave Store" });
        main.name_feeds(json!([declared]));
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        let catalog = &fixture.catalog;

        let view = catalog.view(false).await;
        assert_eq!(ids(&view), ["arthur", "bublik"], "the main's first; its id is not the store's; no stray");
        let (arthur, bublik) = (&view.packages[0], &view.packages[1]);
        assert_eq!(
            (arthur.version.as_str(), arthur.publisher.as_deref(), arthur.source.as_deref()),
            ("1.2.0", Some("duowave"), None)
        );
        assert_eq!((bublik.publisher.as_deref(), bublik.source.as_deref()), (Some("duowave"), Some("DuoWave Store")));
        assert!(arthur.verified && !bublik.verified, "the store's `verified` stands only when the main allows it");
        assert_eq!(bublik.preview.image.as_deref(), Some("/api/v1/packages/catalog/bublik/preview/image"));
        let (image, _) = catalog.preview("bublik", "image").await.unwrap();
        assert!(image.starts_with(catalog.extra_dir(&store.address())), "{}", image.display());
        let feed = &view.feeds[0];
        assert_eq!(
            (feed.url.as_str(), feed.title.as_str(), feed.publisher.as_str()),
            (store.url.as_str(), "DuoWave Store", "duowave")
        );
        assert_eq!((feed.count, feed.offline, feed.problem.as_deref()), (1, false, None));
        assert!(feed.fetched_at.is_some());
        assert_eq!(catalog.install("stray", None, false, None).await.unwrap_err().status, StatusCode::NOT_FOUND);

        // Installed from its own site, and not vouched for.
        let installed = catalog.install("bublik", None, false, None).await.unwrap();
        assert_eq!((installed.source.as_str(), installed.verified), ("catalog", false));
        assert_eq!(catalog.kind_of("bublik").await, Some(PackageKind::Character));
        catalog.remove("bublik", None).await.unwrap();
        // The main allows its `verified`: so it is, and vouched for.
        let mut declared = declared;
        declared["verified"] = json!(true);
        main.name_feeds(json!([declared]));
        let view = catalog.view(true).await;
        assert!(view.packages[1].verified);
        let installed = catalog.install("bublik", None, false, None).await.unwrap();
        assert!(installed.verified);
        assert_eq!(catalog.view(false).await.packages[1].state, EntryState::Installed);
    }

    #[tokio::test]
    async fn never_installs_code_from_an_extra_feed_even_beside_a_signed_main_one() {
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let (main, store) = (site().await, site().await);
        let plugin = |id: &str, phrase: &str| {
            let manifest = json!({
                "schema": 1, "kind": "plugin", "id": id, "version": "1.0.0", "title": "Часы", "api": "^2",
                "runs": ["pc", "station", "stationServer"], "permissions": [],
                "contributes": { "intents": [{ "phrases": [phrase], "handler": "say" }] }
            });
            let mut entry = arthur("1.0.0");
            entry["id"] = json!(id);
            entry["kind"] = json!("plugin");
            (entry, plugin_vgp(&manifest, "export function say() { return 'Полночь'; }"))
        };
        main.publish(&[plugin("clock", "который час на луне")]);
        store.publish(&[plugin("moon", "какая сейчас луна")]);
        // The store vouched for, and the main feed signed: still no code from the store.
        main.name_feeds(json!([{ "url": store.url, "publisher": "duowave", "verified": true }]));
        let text = lock(&main.feed).clone();
        let signature = minisign::sign(Some(&keys.pk), &keys.sk, std::io::Cursor::new(text.as_bytes()), None, None);
        lock(&main.files).insert("catalog.json.minisig".into(), signature.unwrap().to_string().into_bytes());
        let keyed = fixture_keyed(FeedSource::Url(main.url.clone()), Placement::Pc, Some(keys.pk.to_base64()));

        let view = keyed.catalog.view(false).await;
        assert!(view.signed, "{view:?}");
        assert_eq!(view.packages[0].state, EntryState::Available);
        let moon = &view.packages[1];
        assert_eq!((moon.id.as_str(), moon.state), ("moon", EntryState::NeedsSignature));
        let why = moon.reason.clone().unwrap();
        assert!(why.contains("из ленты «duowave»"), "without a title — its publisher: {why}");
        let refused = keyed.catalog.install("moon", None, false, None).await.unwrap_err();
        assert_eq!((refused.status, refused.message), (StatusCode::CONFLICT, why));
        assert!(!keyed.root.join("moon").exists());
        let installed = keyed.catalog.install("clock", None, false, None).await.unwrap();
        assert!(installed.verified, "code from the signed main feed");
    }

    #[tokio::test]
    async fn an_extra_feed_that_fails_leaves_the_others_and_one_no_longer_named_goes_with_its_copy() {
        let (main, store, partner) = (site().await, site().await, site().await);
        main.publish(&[character("arthur", "1.0.0")]);
        store.publish(&[character("bublik", "1.0.0")]);
        partner.publish(&[character("palych", "1.0.0")]);
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let named = |feeds: &[(&str, &str)]| {
            let feeds: Vec<Value> = feeds
                .iter()
                .map(|(url, title)| json!({ "url": url, "publisher": "someone", "title": title }))
                .collect();
            main.name_feeds(json!(feeds));
        };
        let nowhere = format!("http://127.0.0.1:{closed}/catalog.json");
        named(&[
            (store.url.as_str(), "DuoWave Store"),
            (nowhere.as_str(), "Nowhere"),
            (partner.url.as_str(), "Partner"),
        ]);
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        let catalog = &fixture.catalog;

        let view = catalog.view(false).await;
        assert_eq!(ids(&view), ["arthur", "bublik", "palych"]);
        assert!(!view.offline && view.problem.is_none(), "the main feed stands: {view:?}");
        let nowhere = &view.feeds[1];
        assert!(nowhere.offline && nowhere.count == 0 && nowhere.fetched_at.is_none(), "{nowhere:?}");
        assert!(nowhere.problem.as_deref().unwrap().starts_with("Лента «Nowhere» недоступна"));
        // Not read again at every look: with the main feed, in six hours or when asked.
        let reads = main.reads.load(Ordering::SeqCst);
        catalog.view(false).await;
        assert_eq!(main.reads.load(Ordering::SeqCst), reads);

        // The store fails now: its copy before stays, the others as they were.
        store.down.store(true, Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur", "bublik", "palych"]);
        let failed = &view.feeds[0];
        assert_eq!(failed.problem.as_deref(), Some("Лента «DuoWave Store» ответила HTTP 503 Service Unavailable"));
        assert!(!failed.offline && failed.count == 1);
        assert!(view.feeds[2].problem.is_none() && view.feeds[2].count == 1);
        assert!(view.problem.is_none());

        // The main names the store no more: it goes, its copy too. The partner, down meanwhile, keeps its copy and
        // previews — a new copy of the main feed clears only its own folders.
        let gone = catalog.extra_dir(&store.address());
        assert!(gone.join("catalog.json").is_file() && gone.join("bublik/1.0.0").is_dir());
        named(&[(partner.url.as_str(), "Partner")]);
        partner.down.store(true, Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur", "palych"]);
        assert_eq!(view.feeds.len(), 1);
        assert!(view.feeds[0].problem.as_deref().unwrap().starts_with("Лента «Partner» ответила HTTP 503"));
        assert!(!gone.exists(), "{}", gone.display());
        let kept = catalog.extra_dir(&partner.address());
        assert!(kept.join("catalog.json").is_file() && kept.join("palych/1.0.0/image.webp").is_file());
        assert!(catalog.dir.join("arthur/1.0.0/image.webp").is_file(), "the main feed's previews stay");
        partner.down.store(false, Ordering::SeqCst);
        // The owner's switch covers every feed.
        catalog.set_enabled(false, None).unwrap();
        let before = partner.reads.load(Ordering::SeqCst);
        assert_eq!(ids(&catalog.view(true).await), ["arthur", "palych"], "the copies still shown");
        assert_eq!(partner.reads.load(Ordering::SeqCst), before);
    }

    /// A character whose voice is of no known origin, with its archive.
    fn unknown_voice_character(id: &str, version: &str) -> (Value, Vec<u8>) {
        let mut manifest = character_manifest(id, version);
        manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
        let mut entry = arthur(version);
        entry["id"] = json!(id);
        (entry, vgp(&manifest))
    }

    /// The changes told on the bus so far.
    fn changes(events: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let GatewayEvent::PackagesChanged { package_id: id, change, .. } = event.as_ref() {
                seen.push(format!("{id} {change}"));
            }
        }
        seen
    }

    #[test]
    fn a_package_to_hide_needs_its_id_alone() {
        let base = url::Url::parse("https://demo.example/catalog.json").unwrap();
        let text = json!({ "format": 1, "packages": [
            { "id": "jarvis-demo", "hidden": true },
            { "hidden": true },
            { "id": "palych", "hidden": false }
        ]})
        .to_string();
        let feed = parse_feed(&text, &base).unwrap();
        assert_eq!(feed.hidden.iter().collect::<Vec<_>>(), ["jarvis-demo"]);
        assert!(feed.packages.is_empty());
        assert_eq!(feed.skipped.len(), 2, "no id, and an ordinary package without its archive: {:?}", feed.skipped);
        assert!(feed.skipped[0].starts_with("hidden: нет id"));
        // An own source: its host is its title and, in letters an id takes, its publisher; never vouched for.
        let own = own_feed(&url::Url::parse("https://Demo.Duowave.dev/d/x7/catalog.json").unwrap());
        assert_eq!(
            (own.title.as_str(), own.publisher.as_str(), own.verified, own.own),
            ("demo.duowave.dev", "demo-duowave-dev", false, true)
        );
        let local = own_feed(&url::Url::parse("http://127.0.0.1:8781/catalog.json").unwrap());
        assert_eq!((local.title.as_str(), local.publisher.as_str()), ("127.0.0.1:8781", "own"));
    }

    #[tokio::test]
    async fn an_own_source_added_by_its_link_brings_characters_never_code_and_never_an_id_taken() {
        let (main, own) = (site().await, site().await);
        main.publish(&[character("arthur", "1.0.0"), unknown_voice_character("vague", "1.0.0")]);
        let plugin = {
            let manifest = json!({
                "schema": 1, "kind": "plugin", "id": "moon", "version": "1.0.0", "title": "Луна", "api": "^2",
                "runs": ["pc", "station", "stationServer"], "permissions": [],
                "contributes": { "intents": [{ "phrases": ["какая луна"], "handler": "say" }] }
            });
            let mut entry = arthur("1.0.0");
            entry["id"] = json!("moon");
            entry["kind"] = json!("plugin");
            (entry, plugin_vgp(&manifest, "export function say() { return 'Полная'; }"))
        };
        own.publish(&[unknown_voice_character("jarvis-demo", "1.0.0"), character("arthur", "9.0.0"), plugin]);
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        let catalog = &fixture.catalog;
        assert_eq!(ids(&catalog.view(false).await), ["arthur", "vague"]);

        // A link that is no feed is not kept.
        let nowhere = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let refused = catalog.add_source(&format!("http://127.0.0.1:{nowhere}/catalog.json"), None).await.unwrap_err();
        assert_eq!(refused.status, StatusCode::BAD_GATEWAY);
        assert!(refused.message.starts_with("Источник не добавлен"), "{}", refused.message);
        assert!(catalog.sources().is_empty());
        assert_eq!(catalog.add_source(&main.url, None).await.unwrap_err().status, StatusCode::CONFLICT);
        assert_eq!(catalog.add_source("ftp://x/catalog.json", None).await.unwrap_err().status, StatusCode::BAD_REQUEST);

        let view = catalog.add_source(&own.url, None).await.unwrap();
        assert_eq!(
            ids(&view),
            ["arthur", "vague", "jarvis-demo", "moon"],
            "the own source's after; its arthur is not taken"
        );
        let demo = &view.packages[2];
        assert!(demo.own && !demo.verified, "{demo:?}");
        assert_eq!(demo.source.as_deref(), Some(own_feed(&own.address()).title.as_str()));
        assert_eq!(view.packages[3].state, EntryState::NeedsSignature, "never code from an own source");
        let listed = view.feeds.last().unwrap();
        assert!(listed.own && !listed.closed && listed.count == 2, "{listed:?}");
        assert_eq!(catalog.add_source(&own.url, None).await.unwrap_err().status, StatusCode::CONFLICT, "once");
        assert!(catalog.dir.join(SOURCES_FILE).is_file());

        // A voice of no known origin: from the own source, yes; from the catalog, no.
        let installed = catalog.install("jarvis-demo", None, false, None).await.unwrap();
        assert_eq!((installed.source.as_str(), installed.verified), ("catalog", false));
        let noted = registry::choices(&catalog.db)["jarvis-demo"].clone();
        assert_eq!(noted.feed.as_deref(), Some(own.url.as_str()));
        let vague = catalog.install("vague", None, false, None).await.unwrap_err();
        assert_eq!(vague.status, StatusCode::BAD_REQUEST);
        assert!(vague.message.contains("voiceOrigin unknown"), "{}", vague.message);
        assert!(fixture.host.installed("vague").is_none() && !fixture.root.join("vague/1.0.0").exists());
        catalog.install("arthur", None, false, None).await.unwrap();
        assert_eq!(registry::choices(&catalog.db)["arthur"].feed.as_deref(), Some(main.url.as_str()));

        // Up to four.
        let mut sources = catalog.sources();
        for n in 0..3 {
            sources.push(OwnSource { url: format!("https://feed{n}.example/catalog.json"), added_at: now_iso() });
        }
        catalog.write_sources(&sources).unwrap();
        let full = catalog.add_source("https://feed9.example/catalog.json", None).await.unwrap_err();
        assert!(full.message.contains("не больше 4"), "{}", full.message);
    }

    #[tokio::test]
    async fn a_feed_hides_what_was_installed_from_it_and_a_closed_own_source_hides_all_it_gave() {
        let (main, own) = (site().await, site().await);
        main.publish(&[character("arthur", "1.0.0")]);
        own.publish(&[character("jarvis-demo", "1.0.0"), character("palych", "1.0.0")]);
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        let (catalog, host) = (&fixture.catalog, &fixture.host);
        catalog.add_source(&own.url, None).await.unwrap();
        for id in ["arthur", "jarvis-demo", "palych"] {
            catalog.install(id, None, false, None).await.unwrap();
        }
        let mut events = fixture.events.subscribe();
        let listed = |view: PackagesView| view.packages.into_iter().map(|package| package.id).collect::<Vec<_>>();

        // The own source hides one: not in the catalog, not listed here, its files kept.
        let mut entries: Value = serde_json::from_str(&lock(&own.feed)).unwrap();
        entries["packages"].as_array_mut().unwrap()[0] = json!({ "id": "jarvis-demo", "hidden": true });
        *lock(&own.feed) = entries.to_string();
        let view = catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur", "palych"]);
        assert!(host.is_hidden("jarvis-demo") && !host.is_hidden("palych"));
        assert_eq!(listed(host.view().await), ["arthur", "palych"]);
        assert!(host.installed("jarvis-demo").is_some(), "its files stay");
        assert_eq!(host.visible_all().len(), 2);
        assert_eq!(changes(&mut events), ["jarvis-demo hidden"]);
        catalog.view(true).await;
        assert!(changes(&mut events).is_empty(), "told once");

        // The main feed cannot hide what came from the own source; it hides its own.
        main.name_feeds(json!([]));
        let mut feed: Value = serde_json::from_str(&lock(&main.feed)).unwrap();
        feed["packages"].as_array_mut().unwrap().push(json!({ "id": "palych", "hidden": true }));
        feed["packages"].as_array_mut().unwrap()[0] = json!({ "id": "arthur", "hidden": true });
        *lock(&main.feed) = feed.to_string();
        catalog.view(true).await;
        assert!(!host.is_hidden("palych"), "not its feed");
        assert!(host.is_hidden("arthur"));
        main.publish(&[character("arthur", "1.0.0")]);

        // The own source says so no more: shown again.
        own.publish(&[character("jarvis-demo", "1.0.0"), character("palych", "1.0.0")]);
        catalog.view(true).await;
        assert!(!host.is_hidden("jarvis-demo") && !host.is_hidden("arthur"));
        assert_eq!(listed(host.view().await), ["arthur", "jarvis-demo", "palych"]);
        changes(&mut events);

        // Taken down (404): closed — its copy gone, all it gave hidden; told once.
        own.gone.store(true, Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur"]);
        let closed = view.feeds.last().unwrap();
        assert!(closed.own && closed.closed && closed.count == 0, "{closed:?}");
        assert!(closed.problem.as_deref().unwrap().contains("закрыт"), "{closed:?}");
        assert!(host.is_hidden("jarvis-demo") && host.is_hidden("palych"));
        assert!(!catalog.extra_dir(&own.address()).join("catalog.json").exists());
        let mut told = changes(&mut events);
        told.sort();
        assert_eq!(told, ["jarvis-demo hidden", "palych hidden"]);
        catalog.view(true).await;
        assert!(changes(&mut events).is_empty());
        // Down for a while (5xx, no network) is not closed: only 404 and 410.
        own.gone.store(false, Ordering::SeqCst);
        catalog.view(true).await;
        assert!(!host.is_hidden("palych"), "back up: shown again");
        own.down.store(true, Ordering::SeqCst);
        let view = catalog.view(true).await;
        assert!(!view.feeds.last().unwrap().closed && !host.is_hidden("palych"));
        assert_eq!(ids(&view), ["arthur", "jarvis-demo", "palych"], "the copy before stays");

        // Removed: its packages leave the catalog; what came from it stays as it was.
        own.gone.store(true, Ordering::SeqCst);
        own.down.store(false, Ordering::SeqCst);
        catalog.view(true).await;
        let view = catalog.remove_source(&own.url, None).await.unwrap();
        assert!(view.feeds.iter().all(|feed| !feed.own));
        assert!(host.is_hidden("palych"), "still hidden");
        assert!(!catalog.extra_dir(&own.address()).exists());
        assert_eq!(catalog.remove_source(&own.url, None).await.unwrap_err().status, StatusCode::NOT_FOUND);
        // A package put in anew is shown.
        catalog.remove("palych", None).await.unwrap();
        assert!(!host.is_hidden("palych"));
    }

    #[tokio::test]
    async fn a_package_called_feeds_keeps_its_previews_beside_the_extra_feeds_folders() {
        let (main, store) = (site().await, site().await);
        store.publish(&[character("bublik", "1.0.0")]);
        let named = json!([{ "url": store.url, "publisher": "duowave" }]);
        main.publish(&[character("feeds", "1.0.0")]);
        main.name_feeds(named.clone());
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        assert_eq!(ids(&fixture.catalog.view(false).await), ["feeds", "bublik"]);
        // Its newer version: the folder of the one before goes, the store's copy stays while the store is down.
        main.publish(&[character("feeds", "1.1.0")]);
        main.name_feeds(named);
        store.down.store(true, Ordering::SeqCst);
        let view = fixture.catalog.view(true).await;
        assert_eq!((ids(&view), view.packages[0].version.as_str()), (vec!["feeds", "bublik"], "1.1.0"));
        assert!(fixture.catalog.preview("feeds", "image").await.is_ok());
        assert!(fixture.catalog.preview("bublik", "image").await.is_ok());
        assert!(!fixture.catalog.dir.join("feeds/1.0.0").exists());
    }

    #[tokio::test]
    async fn reads_an_extra_feed_at_once_when_the_copy_of_the_main_one_is_fresh_but_older_than_it() {
        let (main, store) = (site().await, site().await);
        main.publish(&[character("arthur", "1.0.0")]);
        store.publish(&[character("bublik", "1.0.0")]);
        let fixture = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        assert_eq!(ids(&fixture.catalog.view(false).await), ["arthur"]);
        // A copy that names the store, written by a gateway that did not read extra feeds — still fresh.
        main.name_feeds(json!([{ "url": store.url, "publisher": "duowave" }]));
        std::fs::write(fixture.catalog.dir.join("catalog.json"), lock(&main.feed).as_bytes()).unwrap();
        assert_eq!(ids(&fixture.catalog.view(false).await), ["arthur", "bublik"]);
        let reads = (main.reads.load(Ordering::SeqCst), store.reads.load(Ordering::SeqCst));
        fixture.catalog.view(false).await;
        assert_eq!((main.reads.load(Ordering::SeqCst), store.reads.load(Ordering::SeqCst)), reads);
    }

    #[tokio::test]
    async fn the_station_sees_the_extra_feeds_through_the_computer() {
        struct Computer(String);
        impl crate::plugins::controller::HomeLink for Computer {
            fn up(&self) -> Option<(String, String)> {
                Some((self.0.clone(), "pair-key".into()))
            }
        }
        async fn serve(router: Router) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            address
        }
        let (main, store) = (site().await, site().await);
        main.publish(&[character("arthur", "1.0.0")]);
        store.publish(&[character("bublik", "1.0.0")]);
        main.name_feeds(json!([{ "url": store.url, "publisher": "duowave", "title": "DuoWave Store" }]));
        // The computer reads both feeds, and hands them on as its home server does.
        let computer = fixture(FeedSource::Url(main.url.clone()), Placement::Pc);
        assert_eq!(ids(&computer.catalog.view(false).await), ["arthur", "bublik"]);
        let copy = computer.catalog.export_for_station().unwrap();
        assert_eq!(
            (copy["feeds"][0]["url"].as_str(), copy["feeds"].as_array().map(Vec::len)),
            (Some(store.url.as_str()), Some(1))
        );
        let archive = format!("{}/packages/bublik-1.0.0.vgp", store.origin());
        assert!(computer.catalog.fetch_for_station(&archive).await.is_ok());
        let elsewhere = computer.catalog.fetch_for_station("http://127.0.0.1:9/packages/x.vgp").await.unwrap_err();
        assert_eq!(elsewhere.status, StatusCode::FORBIDDEN);
        let (export, files) = (computer.catalog.clone(), computer.catalog.clone());
        let home = serve(
            Router::new()
                .route(
                    "/home/v1/catalog",
                    get(move || {
                        let catalog = export.clone();
                        async move { axum::Json(catalog.export_for_station().unwrap()) }
                    }),
                )
                .route(
                    "/home/v1/catalog/file",
                    get(move |axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>| {
                        let catalog = files.clone();
                        async move { catalog.fetch_for_station(&query["url"]).await }
                    }),
                ),
        )
        .await;

        // The station: neither site in its reach.
        let station = fixture_with(
            FeedSource::Url(main.url.clone()),
            Placement::StationServer,
            None,
            Some(Arc::new(Computer(home))),
        );
        main.down.store(true, Ordering::SeqCst);
        store.down.store(true, Ordering::SeqCst);
        let view = station.catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur", "bublik"], "{view:?}");
        assert_eq!(view.packages[1].source.as_deref(), Some("DuoWave Store"));
        assert!(!view.offline && !view.feeds[0].offline && view.feeds[0].count == 1, "{view:?}");
        // The store's archive the computer's way: the station's own try is refused.
        store.refuse.store(1, Ordering::SeqCst);
        let installed = station.catalog.install("bublik", None, false, None).await.unwrap();
        assert_eq!(installed.version.as_deref(), Some("1.0.0"));

        // A computer older than extra feeds sends the main one alone: so the station sees it.
        let text = lock(&main.feed).clone();
        let url = main.url.clone();
        let older = serve(Router::new().route(
            "/home/v1/catalog",
            get(move || {
                let (url, text) = (url.clone(), text.clone());
                async move { axum::Json(json!({ "url": url, "feed": text, "signature": null })) }
            }),
        ))
        .await;
        let station = fixture_with(
            FeedSource::Url(main.url.clone()),
            Placement::StationServer,
            None,
            Some(Arc::new(Computer(older))),
        );
        let view = station.catalog.view(true).await;
        assert_eq!(ids(&view), ["arthur"]);
        assert!(view.feeds[0].count == 0 && !view.feeds[0].offline && view.feeds[0].problem.is_none());
    }
}
