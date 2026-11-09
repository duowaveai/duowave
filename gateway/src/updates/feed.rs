//! The release feed: `latest.json` in each release, in the Tauri updater's format, so one file serves the desktop app
//! and the gateway alone. The app reads its keys (`darwin-aarch64`, `linux-x86_64`, …); the gateway reads its own,
//! `duowave-<os>-<arch>` for a PC or a server and `duowave-station-<arch>` for the build that runs on the station.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Feed {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub platforms: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    /// The minisign signature of the file, as `tauri signer sign` writes it (base64 of the .sig text).
    pub signature: String,
    pub url: String,
}

/// This build's key in the feed.
pub fn platform_key() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        "arm" => "armv7",
        other => other,
    };
    if cfg!(feature = "station") {
        return format!("duowave-station-{arch}");
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("duowave-{os}-{arch}")
}

/// A version as releases write it: `0.2.0` or `v0.2.0`.
pub fn parse_version(text: &str) -> Option<semver::Version> {
    semver::Version::parse(text.trim().trim_start_matches('v')).ok()
}

/// This version's own address for the feed, when it has one: GitHub's own feed `…/releases/latest/download/…`
/// turns into the frozen file of that very release, any other feed at `/latest.json` into the versioned path beside
/// it — where a rotation pins a cohort to the release it can still take. None: no per-version address.
pub fn versioned(feed: &str, version: &str) -> Option<String> {
    if let Some((base, file)) = feed.split_once("/releases/latest/download/") {
        return Some(format!("{base}/releases/download/v{}/{file}", version.trim_start_matches('v')));
    }
    let mut url = url::Url::parse(feed).ok()?;
    if url.path() != "/latest.json" {
        return None;
    }
    url.set_path(&format!("/{}/latest.json", version.trim_start_matches('v')));
    Some(url.to_string())
}

/// The addresses this version's feed is asked at, best first: the version's own (`versioned`), then the feed as
/// given — and a feed already at its versioned path (the app names one to its engine) falls back to the plain one.
/// A versioned address that is missing or not pinned costs nothing: the plain feed answers.
pub fn urls_for(feed: &str, version: &str) -> Vec<String> {
    let mut urls = versioned(feed, version).into_iter().collect::<Vec<_>>();
    urls.push(feed.to_string());
    if let Ok(url) = url::Url::parse(feed) {
        if url.path() == format!("/{}/latest.json", version.trim_start_matches('v')) {
            let mut plain = url;
            plain.set_path("/latest.json");
            urls.push(plain.to_string());
        }
    }
    urls
}

/// This version's frozen feed, found beside another release's files: a feed's assets live at
/// `…/releases/download/v<its version>/<file>`, so `…/releases/download/v<version>/latest.json` is the feed this very
/// version shipped with. That is how a gateway a version behind finds its own release through updates.duowave.dev,
/// which sends every path to the newest release. None: the feed's files are not in a GitHub-style release.
pub fn frozen_beside(feed: &Feed, version: &str) -> Option<String> {
    let (base, _) = feed.platforms.values().find_map(|asset| asset.url.split_once("/releases/download/"))?;
    Some(format!("{base}/releases/download/v{}/latest.json", version.trim_start_matches('v')))
}

/// Whether the feed offers something newer than `current`.
pub fn is_newer(offered: &str, current: &str) -> bool {
    match (parse_version(offered), parse_version(current)) {
        (Some(offered), Some(current)) => offered > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tauri_feed_and_compares_versions() {
        let feed: Feed = serde_json::from_str(
            r#"{"version":"v0.2.0","notes":"Новое","pub_date":"2026-11-06T00:00:00Z",
                "platforms":{"duowave-linux-aarch64":{"signature":"c2ln","url":"https://example.com/duowave.tar.gz"}}}"#,
        )
        .unwrap();
        assert_eq!(feed.platforms["duowave-linux-aarch64"].url, "https://example.com/duowave.tar.gz");
        assert!(is_newer(&feed.version, "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
        assert!(platform_key().starts_with("duowave-"));
    }

    #[test]
    fn the_versions_own_address() {
        assert_eq!(
            versioned("https://github.com/o/r/releases/latest/download/latest.json", "v0.2.0").as_deref(),
            Some("https://github.com/o/r/releases/download/v0.2.0/latest.json"),
            "GitHub's own feed: the frozen file of this very release"
        );
        assert_eq!(
            versioned("https://updates.duowave.dev/latest.json", "0.3.0").as_deref(),
            Some("https://updates.duowave.dev/0.3.0/latest.json"),
            "the versioned path beside the plain feed"
        );
        assert_eq!(versioned("https://updates.duowave.dev/0.3.0/latest.json", "0.3.0"), None, "already versioned");
        assert_eq!(versioned("https://example.com/feed.json", "0.3.0"), None, "no per-version address");
    }

    #[test]
    fn the_versioned_address_first_and_the_plain_feed_last() {
        assert_eq!(
            urls_for("https://updates.duowave.dev/latest.json", "0.3.0"),
            vec![
                "https://updates.duowave.dev/0.3.0/latest.json".to_string(),
                "https://updates.duowave.dev/latest.json".to_string(),
            ]
        );
        assert_eq!(
            urls_for("https://updates.duowave.dev/0.3.0/latest.json", "v0.3.0"),
            vec![
                "https://updates.duowave.dev/0.3.0/latest.json".to_string(),
                "https://updates.duowave.dev/latest.json".to_string(),
            ],
            "a feed already versioned still has the plain one to fall back to"
        );
        assert_eq!(
            urls_for("https://github.com/o/r/releases/latest/download/latest.json", "0.2.0"),
            vec![
                "https://github.com/o/r/releases/download/v0.2.0/latest.json".to_string(),
                "https://github.com/o/r/releases/latest/download/latest.json".to_string(),
            ]
        );
        assert_eq!(
            urls_for("https://example.com/feed.json", "0.3.0"),
            vec!["https://example.com/feed.json".to_string()]
        );
    }

    #[test]
    fn this_versions_frozen_feed_beside_another_releases_files() {
        let feed = |url: &str| -> Feed {
            serde_json::from_value(serde_json::json!({
                "version": "0.2.1",
                "platforms": {"duowave-station-aarch64": {"signature": "x", "url": url}}
            }))
            .unwrap()
        };
        let newest = feed("https://github.com/o/r/releases/download/v0.2.1/duowave-station-aarch64.tar.gz");
        assert_eq!(
            frozen_beside(&newest, "v0.2.0").as_deref(),
            Some("https://github.com/o/r/releases/download/v0.2.0/latest.json")
        );
        let elsewhere = feed("https://files.example.com/duowave-station-aarch64.tar.gz");
        assert_eq!(frozen_beside(&elsewhere, "0.2.0"), None, "not a GitHub-style release");
    }
}
