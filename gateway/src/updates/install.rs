//! Installing an update of the gateway alone (a PC, a server, the station): the archive is downloaded into the data
//! folder, its signature checked, and the program inside it replaces the running one by a rename. The program before
//! stays beside it as `duowave.previous` — the way back — and the gateway starts again as the new program, in the same
//! process where the system allows it (exec), so a service manager sees no exit.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An update larger than this is not one of ours.
pub const MAX_DOWNLOAD: usize = 96 * 1024 * 1024;

/// The archive at `url`, whole, into memory (the gateway is ~10–30 MB). Over https only, each redirect too
/// (`feed::secure`).
pub async fn download(url: &str) -> Result<Vec<u8>, String> {
    super::feed::secure(url)?;
    let client = super::feed::client_builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| format!("загрузка: {error}"))?;
    let response = client.get(url).send().await.map_err(|error| format!("загрузка: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("загрузка: HTTP {}", response.status()));
    }
    if response.content_length().is_some_and(|length| length as usize > MAX_DOWNLOAD) {
        return Err("download: the file is too large for an update".into());
    }
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|error| format!("загрузка: {error}"))? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_DOWNLOAD {
            return Err("download: the file is too large for an update".into());
        }
    }
    Ok(bytes)
}

/// The program's file name in an archive.
pub fn program_name() -> &'static str {
    if cfg!(windows) {
        "duowave.exe"
    } else {
        "duowave"
    }
}

/// The program inside a `.tar.gz`: a file named `duowave` (`duowave.exe`) at the top or in one folder.
pub fn unpack(archive: &[u8]) -> Result<Vec<u8>, String> {
    unpack_named(archive, program_name())
}

/// The program named `name` inside a `.tar.gz`: the station's build is `duowave` whatever the computer's system.
pub fn unpack_named(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let program = find(archive, name, u64::MAX)?.ok_or_else(|| format!("в архиве нет {name}"))?;
    if program.len() < 1024 {
        return Err("archive: the program in it is empty".into());
    }
    Ok(program)
}

/// The file named `name` inside a `.tar.gz`, at the top or in one folder, when there is one; one larger than `limit`
/// is an error, not read.
fn find(archive: &[u8], name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    with_file(archive, name, |entry| {
        if entry.size() > limit {
            return Err(format!("архив: {name} слишком велик"));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| format!("архив: {error}"))?;
        Ok(bytes)
    })
}

/// `take` run on the file named `name` inside a `.tar.gz`, at the top or in one folder, as it streams out of the
/// archive; None when there is no such file.
fn with_file<T>(
    archive: &[u8],
    name: &str,
    take: impl FnOnce(&mut tar::Entry<'_, flate2::read::GzDecoder<&[u8]>>) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = tar.entries().map_err(|error| format!("архив: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("архив: {error}"))?;
        let path = entry.path().map_err(|error| format!("архив: {error}"))?.into_owned();
        let file = path.file_name().and_then(|file| file.to_str()).unwrap_or("");
        if file == name && path.components().count() <= 2 && entry.header().entry_type().is_file() {
            return take(&mut entry).map(Some);
        }
    }
    Ok(None)
}

/// What the release workflow writes beside the program in every archive: the version and the platform it was built
/// as. The archive's signature covers it with the program, so both are the signer's word, not the feed's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Release {
    pub version: String,
    pub platform: String,
}

/// Its name in the archive, beside the program.
pub const RELEASE_FILE: &str = "release.json";

/// The release a signed archive says it is, checked against what it is about to be installed as: built for this
/// platform, the very version offered, and newer than the one running. The feed is not signed, so it could offer an
/// old signed archive as a new version and take a gateway back to a release with a known hole; or another platform's
/// build, which would not start. Both stop here. Archives before 0.2.1 carry no release.json, so no gateway that
/// checks ever installs one of them again.
pub fn check_release(archive: &[u8], platform: &str, offered: &str, current: &str) -> Result<Release, String> {
    use super::feed::{is_newer, parse_version};
    let text = find(archive, RELEASE_FILE, 64 * 1024)?.ok_or_else(|| {
        format!("в архиве нет {RELEASE_FILE}: это выпуск до 0.2.1 или собранный не конвейером выпуска")
    })?;
    let release: Release =
        serde_json::from_slice(&text).map_err(|error| format!("{RELEASE_FILE} не читается: {error}"))?;
    if release.platform != platform {
        return Err(format!("архив собран для {}, а здесь {platform}", release.platform));
    }
    let (Some(signed), Some(wanted)) = (parse_version(&release.version), parse_version(offered)) else {
        return Err(format!("версия не читается: в архиве «{}», предложена «{offered}»", release.version));
    };
    if signed != wanted {
        return Err(format!("архив подписан как версия {signed}, а предложен как {wanted}"));
    }
    if !is_newer(&release.version, current) {
        return Err(format!("версия {signed} не новее стоящей {current}: откат на прежнюю версию не ставится"));
    }
    Ok(release)
}

/// Where the program before an update is kept.
pub fn previous_of(current: &Path) -> PathBuf {
    let name = current.file_name().and_then(|name| name.to_str()).unwrap_or("duowave");
    current.with_file_name(format!("{name}.previous"))
}

/// Puts `program` in place of `current`: written beside it, made executable, then renamed over it, the old one
/// kept as `<name>.previous`. A rename is atomic: a failure midway leaves one of the two programs in place, never
/// half of one.
pub fn replace(current: &Path, program: &[u8]) -> std::io::Result<PathBuf> {
    let incoming = incoming_of(current);
    std::fs::write(&incoming, program)?;
    swap_in(current, &incoming)
}

/// The program named `name` inside a `.tar.gz` put in place of `current` as `replace` puts it, but streamed from the
/// archive to the disk: never whole in memory. A Station Lite 2 has some 90 MB to spare; the update that held the
/// 16 MB program in memory — beside the archive, which came as base64 in JSON — got the station's gateway killed by
/// the kernel midway, twice on 11.11.2026 (0.4.0 and 0.5.0), and it came back as the old version. Nothing is left
/// half-written: a failure removes the incoming file and the running program stays.
pub fn replace_from_archive(current: &Path, archive: &[u8], name: &str) -> Result<PathBuf, String> {
    let incoming = incoming_of(current);
    let on_disk = |error: std::io::Error| format!("{}: {error}", incoming.display());
    let written = with_file(archive, name, |entry| {
        let mut file = std::fs::File::create(&incoming).map_err(on_disk)?;
        let written = std::io::copy(entry, &mut file).map_err(|error| format!("архив: {error}"))?;
        file.sync_all().map_err(on_disk)?;
        Ok(written)
    });
    let outcome = match written {
        Ok(Some(written)) if written >= 1024 => {
            swap_in(current, &incoming).map_err(|error| format!("{}: {error}", current.display()))
        }
        Ok(Some(_)) => Err("archive: the program in it is empty".to_string()),
        Ok(None) => Err(format!("в архиве нет {name}")),
        Err(error) => Err(error),
    };
    if outcome.is_err() {
        let _ = std::fs::remove_file(&incoming);
    }
    outcome
}

/// Where a new program is written before it takes the running one's place.
fn incoming_of(current: &Path) -> PathBuf {
    current.with_file_name(format!("{}.new", program_name()))
}

/// `incoming` made executable and renamed over `current`, the old one kept as `<name>.previous`.
fn swap_in(current: &Path, incoming: &Path) -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(incoming, std::fs::Permissions::from_mode(0o755))?;
    }
    let previous = previous_of(current);
    let _ = std::fs::remove_file(&previous);
    // A running program may be renamed (Windows too), not overwritten.
    std::fs::rename(current, &previous)?;
    if let Err(error) = std::fs::rename(incoming, current) {
        let _ = std::fs::rename(&previous, current);
        return Err(error);
    }
    Ok(previous)
}

/// Starts the gateway again as the program now at `current`, with the same arguments: on Unix the process becomes the
/// new program (exec); elsewhere the new one starts and this one exits. Returns only on failure.
pub fn restart(current: &Path) -> std::io::Error {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(current).args(&args).exec()
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(current).args(&args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn finds_the_program_in_the_archive() {
        let program = vec![7u8; 4096];
        assert_eq!(unpack(&archive(&[("README.md", b"x"), (program_name(), &program)])).unwrap(), program);
        let folder = format!("duowave-0.2.0/{}", program_name());
        assert_eq!(unpack(&archive(&[(&folder, &program)])).unwrap(), program);
        assert!(unpack(&archive(&[("other", &program)])).is_err());
        assert!(unpack(&archive(&[(program_name(), b"tiny")])).is_err());
        assert!(unpack(b"not an archive").is_err());
    }

    #[test]
    fn a_release_installs_only_as_itself_for_this_platform_and_forward() {
        let program = vec![7u8; 4096];
        let here = "duowave-linux-x86_64";
        let release = |version: &str, platform: &str| {
            archive(&[
                (program_name(), &program),
                (RELEASE_FILE, format!(r#"{{"version":"{version}","platform":"{platform}"}}"#).as_bytes()),
            ])
        };
        assert_eq!(check_release(&release("0.2.2", here), here, "0.2.2", "0.2.1").unwrap().version, "0.2.2");
        assert!(check_release(&release("0.2.2", here), here, "v0.2.2", "0.2.1").is_ok(), "the feed's v");
        let replayed = check_release(&release("0.2.0", here), here, "99.0.0", "0.2.1").unwrap_err();
        assert!(replayed.contains("0.2.0") && replayed.contains("99.0.0"), "an old release offered as new: {replayed}");
        assert!(check_release(&release("0.2.1", here), here, "0.2.1", "0.2.1").is_err(), "the same version");
        assert!(check_release(&release("0.2.0", here), here, "0.2.0", "0.2.1").is_err(), "an older one, as itself");
        assert!(
            check_release(&release("0.2.2", "duowave-linux-aarch64"), here, "0.2.2", "0.2.1").is_err(),
            "another platform"
        );
        let unmarked = check_release(&archive(&[(program_name(), &program)]), here, "0.2.2", "0.2.1").unwrap_err();
        assert!(unmarked.contains(RELEASE_FILE), "a release before 0.2.1: {unmarked}");
        // The program is still found beside it: gateways before 0.2.1 take the new archives as before.
        assert_eq!(unpack(&release("0.2.2", here)).unwrap(), program);
    }

    #[test]
    fn replaces_the_program_and_keeps_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(program_name());
        std::fs::write(&current, b"old").unwrap();
        let previous = replace(&current, b"new program").unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"new program");
        assert_eq!(std::fs::read(&previous).unwrap(), b"old");
        assert_eq!(previous, previous_of(&current));
        replace(&current, b"newer").unwrap();
        assert_eq!(std::fs::read(&previous).unwrap(), b"new program", "only the last one is kept");
    }

    #[test]
    fn puts_the_program_in_place_straight_from_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(program_name());
        std::fs::write(&current, b"old").unwrap();
        let program: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let folder = format!("duowave-0.5.1/{}", program_name());
        let archived = archive(&[("README.md", b"x"), (&folder, &program)]);
        let previous = replace_from_archive(&current, &archived, program_name()).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), program);
        assert_eq!(std::fs::read(&previous).unwrap(), b"old");
        // Nothing fit to put in place: the running program stays, and nothing half-written is left beside it.
        assert!(replace_from_archive(&current, &archive(&[("other", &program)]), program_name()).is_err());
        assert!(replace_from_archive(&current, &archive(&[(program_name(), b"tiny")]), program_name()).is_err());
        assert!(replace_from_archive(&current, b"not an archive", program_name()).is_err());
        let cut = &archived[..archived.len() / 2];
        assert!(replace_from_archive(&current, cut, program_name()).is_err(), "an archive cut short");
        assert_eq!(std::fs::read(&current).unwrap(), program);
        assert!(!incoming_of(&current).exists());
    }
}
