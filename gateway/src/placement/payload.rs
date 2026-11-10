//! The station's build inside the computer's (docs/plans/station-placement.md, section 2): moving the gateway onto
//! the station from the panel needs no file to find. A release builds the station first and every computer build
//! carries its signed archive (gateway/build.rs, DUOWAVE_STATION_PAYLOAD). A local build carries none: then the
//! station's build of this very version comes from the release feed, or from STATION_PAYLOAD_FILE. Whatever its
//! source, its signature is checked with the updates' key before anything goes to a station.

use serde::Serialize;

use crate::config::Config;
use crate::updates::{feed_url, verify};

mod built {
    include!(concat!(env!("OUT_DIR"), "/station_payload.rs"));
}

/// The station's key in the release feed.
pub const STATION_PLATFORM: &str = "duowave-station-aarch64";
/// The program's name in the station's archive, whatever this computer's system.
pub const STATION_PROGRAM: &str = "duowave";

/// Where the station's build comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PayloadSource {
    /// Inside this program.
    BuiltIn,
    /// STATION_PAYLOAD_FILE.
    File,
    /// The release feed of this very version.
    Feed,
    /// None: the station cannot be given a gateway from here.
    None,
}

/// What the panel says about it, before anything is downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadInfo {
    pub source: PayloadSource,
    pub version: &'static str,
    pub note: String,
}

/// The station's build, its signature checked.
pub struct Payload {
    pub archive: Vec<u8>,
    pub signature: String,
    pub version: String,
    pub source: PayloadSource,
}

impl Payload {
    /// The program inside, for a station (aarch64 Linux).
    pub fn program(&self) -> Result<Vec<u8>, String> {
        crate::updates::install::unpack_named(&self.archive, STATION_PROGRAM)
    }
}

pub fn built_in() -> Option<(&'static [u8], &'static str)> {
    built::BUILT_IN
}

/// Where the station's build would come from, without fetching it.
pub fn info(config: &Config) -> PayloadInfo {
    let version = crate::VERSION;
    if built_in().is_some() {
        return PayloadInfo {
            source: PayloadSource::BuiltIn,
            version,
            note: format!("Сборка для колонки {version} встроена в этот шлюз."),
        };
    }
    if let Some(file) = &config.placement.station_payload_file {
        return PayloadInfo {
            source: PayloadSource::File,
            version,
            note: format!("Сборка для колонки — из файла {}.", file.display()),
        };
    }
    match feed_url(config).and_then(|feed| crate::updates::feed::versioned(&feed, version)) {
        Some(_) => PayloadInfo {
            source: PayloadSource::Feed,
            version,
            note: format!("Сборку для колонки {version} шлюз скачает из выпуска и проверит её подпись."),
        },
        None => PayloadInfo {
            source: PayloadSource::None,
            version,
            note: "В этой сборке шлюза нет сборки для колонки, и лента выпусков не задана: укажите файл \
                   STATION_PAYLOAD_FILE или поставьте выпуск."
                .into(),
        },
    }
}

/// The station's build: built in, else the file, else the release of this very version; its signature checked.
pub async fn load(config: &Config) -> Result<Payload, String> {
    let keys = verify::public_keys(config.updates.public_key.as_deref().unwrap_or(verify::BUILT_IN_KEY))?;
    let (archive, signature, source) = if let Some((archive, signature)) = built_in() {
        (archive.to_vec(), signature.to_string(), PayloadSource::BuiltIn)
    } else if let Some(file) = &config.placement.station_payload_file {
        let archive = std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let signature_file = std::path::PathBuf::from(format!("{}.sig", file.display()));
        let signature = std::fs::read_to_string(&signature_file)
            .map_err(|error| format!("{}: {error}", signature_file.display()))?;
        (archive, signature, PayloadSource::File)
    } else {
        let plain = feed_url(config).ok_or_else(|| info(config).note)?;
        if crate::updates::feed::versioned(&plain, crate::VERSION).is_none() {
            return Err(info(config).note);
        }
        let release = release_of_this_version(&plain).await?;
        let asset = release
            .platforms
            .get(STATION_PLATFORM)
            .ok_or_else(|| format!("в выпуске {} нет сборки для колонки", crate::VERSION))?;
        let archive = crate::updates::install::download(&asset.url).await?;
        (archive, asset.signature.clone(), PayloadSource::Feed)
    };
    verify::verify_any(&keys, &archive, &signature)?;
    let payload = Payload { archive, signature, version: crate::VERSION.to_string(), source };
    payload.program()?;
    Ok(payload)
}

/// The feed of this very version. Its own address first (GitHub's frozen feed of its release), the plain feed after
/// it — and only a feed of this version counts: updates.duowave.dev sends every path to the newest release, so a
/// gateway a version behind gets the newest feed there and finds its own release beside the newest one's files.
async fn release_of_this_version(plain: &str) -> Result<crate::updates::Feed, String> {
    use crate::updates::feed::{frozen_beside, parse_version, urls_for};
    let wanted = parse_version(crate::VERSION);
    let mut other: Option<crate::updates::Feed> = None;
    let mut error: Option<String> = None;
    for url in urls_for(plain, crate::VERSION) {
        match fetch_feed(&url).await {
            Ok(feed) if parse_version(&feed.version) == wanted => return Ok(feed),
            Ok(feed) => other = other.or(Some(feed)),
            Err(why) => error = error.or(Some(why)),
        }
    }
    let Some(other) = other else {
        return Err(error.unwrap_or_else(|| "лента выпуска недоступна".to_string()));
    };
    if let Some(frozen) = frozen_beside(&other, crate::VERSION) {
        if let Ok(feed) = fetch_feed(&frozen).await {
            if parse_version(&feed.version) == wanted {
                return Ok(feed);
            }
        }
    }
    Err(format!("в выпуске {} другая версия, нужна {}", other.version, crate::VERSION))
}

async fn fetch_feed(url: &str) -> Result<crate::updates::Feed, String> {
    crate::updates::feed::secure(url).map_err(|error| format!("лента выпуска: {error}"))?;
    let response = crate::updates::feed::client()
        .get(url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("лента выпуска: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("лента выпуска: HTTP {}", response.status()));
    }
    response.json().await.map_err(|error| format!("лента выпуска: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;

    #[test]
    fn finds_the_release_of_this_very_version() {
        // The addresses themselves are feed::versioned's, tested there; here that the source follows them.
        let versioned =
            Config::for_tests(&Env::from_pairs([("UPDATE_FEED_URL", "https://updates.duowave.dev/latest.json")]));
        assert_eq!(info(&versioned).source, PayloadSource::Feed);
    }

    #[test]
    fn says_where_the_stations_build_comes_from() {
        if built_in().is_some() {
            return;
        }
        // A feed at /latest.json has a versioned address now; one at any other path does not.
        let none = Config::for_tests(&Env::from_pairs([("UPDATE_FEED_URL", "https://updates.example.com/feed.json")]));
        assert_eq!(info(&none).source, PayloadSource::None);
        let feed = Config::for_tests(&Env::from_pairs([(
            "UPDATE_FEED_URL",
            "https://github.com/o/r/releases/latest/download/latest.json",
        )]));
        assert_eq!(info(&feed).source, PayloadSource::Feed);
        let file =
            Config::for_tests(&Env::from_pairs([("STATION_PAYLOAD_FILE", "/tmp/duowave-station-aarch64.tar.gz")]));
        assert_eq!(info(&file).source, PayloadSource::File);
    }

    #[tokio::test]
    async fn never_takes_a_file_whose_signature_does_not_match() {
        if built_in().is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("duowave-station-aarch64.tar.gz");
        std::fs::write(&file, b"not the station's build").unwrap();
        std::fs::write(dir.path().join("duowave-station-aarch64.tar.gz.sig"), "untrusted comment: x\nRUQ=\n").unwrap();
        let config =
            Config::for_tests(&Env::from_pairs([("STATION_PAYLOAD_FILE", file.to_string_lossy().into_owned())]));
        assert!(load(&config).await.is_err());
    }

    #[test]
    fn the_built_in_build_holds_the_stations_program() {
        let Some((archive, signature)) = built_in() else { return };
        assert!(!signature.is_empty());
        let program = crate::updates::install::unpack_named(archive, STATION_PROGRAM).expect("duowave inside");
        assert!(program.starts_with(b"\x7fELF"), "a Linux program");
        assert_eq!(
            info(&Config::for_tests(&Env::from_pairs::<[(&str, &str); 0], _, _>([]))).source,
            PayloadSource::BuiltIn
        );
    }
}
