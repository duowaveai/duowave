//! Mirrors of the packages' feeds (docs/plans/package-mirror.md, «Final shape»): another host that serves the same
//! files as a feed's own one, for homes where that host does not answer or stalls (Russia since 2025: connections to
//! many foreign networks are cut after the first few kilobytes, and the download hangs instead of failing).
//!
//! The address stays the feed's own everywhere: the registry, the pins, the folders, `hidden`. Only the bytes may come
//! from a mirror, and what came is checked as if it came from the feed: archives by the hash their feed or the signed
//! catalog gives. A mirror can hold a file back or be late; it cannot make the gateway take a file it would refuse.
//!
//! The map «feed's prefix → mirror's prefix» comes from MIRRORS in `.env` (`off`: none), else from the release's
//! secret DUOWAVE_MIRRORS. A mirror would lead anyone to the closed feed it serves, so its address is kept as secret as
//! that feed's: in no code and no public file.
//!
//! The feed's own host is asked first. After a failure on the way, a server's error or a stall of `STALL`, the mirror
//! is asked. The one that answered last is remembered for a week (`mirrors.json` in the catalog's folder): a home
//! where the feed's host stalls asks the mirror first from then on and does not wait every time.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The map a release carries; none in a local build.
const BUILT_IN: Option<&str> = option_env!("DUOWAVE_MIRRORS");
/// No byte for this long: the download is taken as stalled and the next source is asked. A cut connection sends
/// nothing at all, and a working one, however slow, is hardly ever silent this long (owner 24.11: 10 s).
pub const STALL: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(10) };
/// A mirror that answered is asked first for this long.
const REMEMBERED_DAYS: i64 = 7;
/// A mirror's answer is written down again at most this often: a station's small disk is not written at every file.
const REWRITE_HOURS: i64 = 24;
/// The memory's file, in the catalog's folder.
pub const MEMORY: &str = "mirrors.json";

/// One feed's prefix and its mirror's: a file at `own` + rest is also at `mirror` + rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub own: String,
    pub mirror: String,
}

/// A place to ask for a file: its address and whether it is a mirror's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub url: url::Url,
    pub mirror: bool,
    /// The feed's prefix it belongs to: what the memory is kept by. Empty for an address no pair covers.
    prefix: String,
}

/// The map and the memory of which source answered.
pub struct Mirrors {
    pairs: Vec<Pair>,
    memory: Option<PathBuf>,
    /// By the feed's prefix: when its mirror last answered.
    first: Mutex<BTreeMap<String, String>>,
}

/// The map's text: pairs «feed's address» «mirror's address», separated by whitespace, the pairs by `;` or a new line.
/// An address may be a folder (`…/`) or a file in it (`…/catalog.json`): its folder is the prefix. HTTPS only, HTTP
/// only on this computer (a test's pretend mirror).
pub fn parse(text: &str) -> Result<Vec<Pair>, String> {
    let mut pairs = Vec::new();
    for line in text.split([';', '\n']).map(str::trim).filter(|line| !line.is_empty()) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [own, mirror] = parts[..] else {
            return Err(format!("«{line}»: a pair is two addresses, the feed's and its mirror's"));
        };
        pairs.push(Pair { own: prefix(own)?, mirror: prefix(mirror)? });
    }
    Ok(pairs)
}

/// The folder of an address, as text ending with `/`.
fn prefix(text: &str) -> Result<String, String> {
    let mut url = url::Url::parse(text).map_err(|_| format!("«{text}» is not an address"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local)) {
        return Err(format!("«{text}»: HTTPS only (HTTP only on localhost)"));
    }
    url.set_query(None);
    url.set_fragment(None);
    let folder = match url.path().rfind('/') {
        Some(at) => url.path()[..=at].to_string(),
        None => "/".to_string(),
    };
    url.set_path(&folder);
    Ok(url.to_string())
}

impl Mirrors {
    /// The map from MIRRORS (`setting`), else the release's; the memory in `memory` when given.
    pub fn new(setting: Option<&str>, memory: Option<PathBuf>) -> Mirrors {
        Mirrors::with(setting, BUILT_IN, memory)
    }

    fn with(setting: Option<&str>, built_in: Option<&str>, memory: Option<PathBuf>) -> Mirrors {
        let text = match setting.map(str::trim).filter(|text| !text.is_empty()) {
            Some(off) if off.eq_ignore_ascii_case("off") => None,
            Some(text) => Some(text),
            None => built_in.map(str::trim).filter(|text| !text.is_empty()),
        };
        let pairs = match text.map(parse) {
            None => Vec::new(),
            Some(Ok(pairs)) => pairs,
            Some(Err(problem)) => {
                tracing::warn!(target: "plugins", "mirrors: {problem}; downloads go to the feeds' own hosts only");
                Vec::new()
            }
        };
        let first = memory
            .as_ref()
            .filter(|_| !pairs.is_empty())
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, String>>(&bytes).ok())
            .unwrap_or_default();
        Mirrors { pairs, memory, first: Mutex::new(first) }
    }

    /// Whether any feed has a mirror.
    pub fn any(&self) -> bool {
        !self.pairs.is_empty()
    }

    /// Where to ask for `url`, in the order to ask: the feed's own address and its mirror's. The address alone when no
    /// pair covers it.
    pub fn sources(&self, url: &url::Url) -> Vec<Source> {
        let text = url.as_str();
        let mirrors: Vec<Source> = self
            .pairs
            .iter()
            .filter(|pair| text.starts_with(&pair.own))
            .filter_map(|pair| {
                let url = url::Url::parse(&format!("{}{}", pair.mirror, &text[pair.own.len()..])).ok()?;
                Some(Source { url, mirror: true, prefix: pair.own.clone() })
            })
            .collect();
        let Some(first) = mirrors.first() else {
            return vec![Source { url: url.clone(), mirror: false, prefix: String::new() }];
        };
        let own = Source { url: url.clone(), mirror: false, prefix: first.prefix.clone() };
        if self.mirror_first(&own.prefix) {
            mirrors.into_iter().chain(std::iter::once(own)).collect()
        } else {
            std::iter::once(own).chain(mirrors).collect()
        }
    }

    /// The source that answered, written down for its feed: a mirror's is asked first for a week; the feed's own host
    /// that answered again makes the mirror second again.
    pub fn answered(&self, source: &Source) {
        if source.prefix.is_empty() {
            return;
        }
        let now = chrono::Utc::now();
        let mut first = self.first.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = if source.mirror {
            let fresh = first
                .get(&source.prefix)
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .is_some_and(|at| now.signed_duration_since(at) < chrono::Duration::hours(REWRITE_HOURS));
            if !fresh {
                first.insert(source.prefix.clone(), crate::utils::time::iso(now));
            }
            !fresh
        } else {
            first.remove(&source.prefix).is_some()
        };
        if changed {
            if let Some(path) = &self.memory {
                let written = serde_json::to_vec(&*first).map_err(std::io::Error::other).and_then(|bytes| {
                    let beside = path.with_extension("tmp");
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&beside, bytes)?;
                    std::fs::rename(&beside, path)
                });
                if let Err(error) = written {
                    tracing::warn!(target: "plugins", "mirrors: {}: {error}", path.display());
                }
            }
        }
    }

    /// Whether the feed's mirror answered within the last week.
    fn mirror_first(&self, prefix: &str) -> bool {
        let first = self.first.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        first
            .get(prefix)
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| chrono::Utc::now().signed_duration_since(at) < chrono::Duration::days(REMEMBERED_DAYS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> url::Url {
        url::Url::parse(text).unwrap()
    }

    fn addresses(sources: &[Source]) -> Vec<(&str, bool)> {
        sources.iter().map(|source| (source.url.as_str(), source.mirror)).collect()
    }

    #[test]
    fn a_map_reads_pairs_by_folder_and_refuses_what_is_not_one() {
        let pairs = parse(
            "https://feed.example/d-1/catalog.json https://m.example/d-1/catalog.json;\n\
             https://store.example/  https://m.example/store/\n",
        )
        .unwrap();
        assert_eq!(
            pairs,
            [
                Pair { own: "https://feed.example/d-1/".into(), mirror: "https://m.example/d-1/".into() },
                Pair { own: "https://store.example/".into(), mirror: "https://m.example/store/".into() },
            ]
        );
        assert_eq!(parse("http://127.0.0.1:5/catalog.json http://localhost:6/x/").unwrap().len(), 1);
        assert!(parse("http://feed.example/ https://m.example/").is_err(), "HTTP off this computer");
        assert!(parse("https://feed.example/ http://m.example/").is_err());
        assert!(parse("https://feed.example/").is_err(), "one address is not a pair");
        assert!(parse("https://a.example/ https://b.example/ https://c.example/").is_err());
        assert!(parse("feed mirror").is_err());
        assert!(parse(" ; \n").unwrap().is_empty());
    }

    #[test]
    fn only_a_covered_address_gets_its_mirror_and_the_rest_of_it_is_kept() {
        let mirrors = Mirrors::with(Some("https://feed.example/d-1/ https://m.example/x/"), None, None);
        let sources = mirrors.sources(&url("https://feed.example/d-1/packages/a-1.0.0.vgp?v=2"));
        assert_eq!(
            addresses(&sources),
            [
                ("https://feed.example/d-1/packages/a-1.0.0.vgp?v=2", false),
                ("https://m.example/x/packages/a-1.0.0.vgp?v=2", true)
            ]
        );
        let other = mirrors.sources(&url("https://feed.example/d-2/catalog.json"));
        assert_eq!(addresses(&other), [("https://feed.example/d-2/catalog.json", false)]);
        let main = mirrors.sources(&url("https://packages.example/catalog.json"));
        assert_eq!(main.len(), 1);
    }

    #[test]
    fn the_setting_replaces_the_releases_map_and_off_turns_both_off() {
        let built_in = Some("https://feed.example/ https://m.example/");
        assert!(Mirrors::with(None, built_in, None).any());
        assert!(Mirrors::with(Some(""), built_in, None).any(), "empty is unset");
        assert!(!Mirrors::with(Some("OFF"), built_in, None).any());
        let own = Mirrors::with(Some("https://feed.example/ https://other.example/"), built_in, None);
        assert_eq!(own.sources(&url("https://feed.example/a"))[1].url.as_str(), "https://other.example/a");
        assert!(!Mirrors::with(Some("garbage"), built_in, None).any(), "a broken map: none, not the release's");
        assert!(!Mirrors::with(None, None, None).any());
    }

    #[test]
    fn a_mirror_that_answered_is_asked_first_for_a_week_and_kept_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let memory = dir.path().join(MEMORY);
        let map = Some("https://feed.example/ https://m.example/");
        let mirrors = Mirrors::with(map, None, Some(memory.clone()));
        let file = url("https://feed.example/catalog.json");
        let sources = mirrors.sources(&file);
        assert!(!sources[0].mirror);

        mirrors.answered(&sources[1]);
        assert!(mirrors.sources(&file)[0].mirror, "the mirror first now");
        assert!(memory.is_file());
        let again = Mirrors::with(map, None, Some(memory.clone()));
        assert!(again.sources(&file)[0].mirror, "after a restart too");

        // The feed's own host answered again: the mirror is second again.
        let own = again.sources(&file).into_iter().find(|source| !source.mirror).unwrap();
        again.answered(&own);
        assert!(!again.sources(&file)[0].mirror);
        assert!(!Mirrors::with(map, None, Some(memory.clone())).sources(&file)[0].mirror);

        // A week old: forgotten.
        let old = crate::utils::time::iso(chrono::Utc::now() - chrono::Duration::days(8));
        std::fs::write(&memory, serde_json::json!({ "https://feed.example/": old }).to_string()).unwrap();
        assert!(!Mirrors::with(map, None, Some(memory)).sources(&file)[0].mirror);
    }

    #[test]
    fn an_address_no_pair_covers_is_never_written_down() {
        let dir = tempfile::tempdir().unwrap();
        let memory = dir.path().join(MEMORY);
        let mirrors = Mirrors::with(Some("https://feed.example/ https://m.example/"), None, Some(memory.clone()));
        let alone = mirrors.sources(&url("https://packages.example/catalog.json"));
        mirrors.answered(&alone[0]);
        assert!(!memory.exists());
    }
}
