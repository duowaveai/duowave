//! Installing an update of the gateway alone (a PC, a server, the station): the archive is downloaded into the data
//! folder, its signature checked, and the program inside it replaces the running one by a rename. The program before
//! stays beside it as `duowave.previous` — the way back — and the gateway starts again as the new program, in the same
//! process where the system allows it (exec), so a service manager sees no exit.
//!
//! On the station the new program is tried from memory first (`updates::trial`, docs/plans/station-update-space.md):
//! `unpack_to` puts it in /tmp, `answers_version` asks it its version, and after the trial `move_to_disk` puts it in
//! the place of the one on the disk — never beside it for long: /data has some 30 MB to spare.
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// An update larger than this is not one of ours.
pub const MAX_DOWNLOAD: usize = 96 * 1024 * 1024;

/// The room an update keeps on the station: the base floor's (core/build.rs), the apps' storage reserve reads it.
pub use crate::core::build::{growth_kb, update_reserve_kb, STATION_FLOOR_KB};

/// The size of the program named `name` inside a `.tar.gz`, as its header says: known before a byte of it is written.
pub fn program_size(archive: &[u8], name: &str) -> Result<u64, String> {
    let size = with_file(archive, name, |entry| Ok(entry.size()))?
        .ok_or_else(|| crate::tr!("В архиве нет {name}.", name = name))?;
    if size < 1024 {
        return Err("archive: the program in it is empty".into());
    }
    Ok(size)
}

/// What updates left beside the program that nothing uses: the version the station's safety net put aside
/// (`<name>.failed`, station-client/gateway.sh — 20 MB nobody ever removed), a new program a failure did not get to
/// remove (`duowave.new`), the temporary file of a copy by hand (`.<name>.new`). Never `.previous`: while it is kept
/// it is the way back.
pub fn leftovers(program: &Path) -> Vec<PathBuf> {
    let name = program.file_name().and_then(|name| name.to_str()).unwrap_or("duowave");
    vec![
        program.with_file_name(format!("{name}.failed")),
        incoming_of(program),
        program.with_file_name(format!(".{name}.new")),
    ]
}

/// The leftovers removed: the bytes freed.
pub fn remove_leftovers(program: &Path) -> u64 {
    leftovers(program).iter().map(|path| remove_counted(path)).sum()
}

/// A file removed: its size, or 0 when there was none.
pub fn remove_counted(path: &Path) -> u64 {
    let size = std::fs::symlink_metadata(path).map(|meta| meta.len()).unwrap_or(0);
    match std::fs::remove_file(path) {
        Ok(()) => size,
        Err(_) => 0,
    }
}

/// Whether two paths name the same file: /data is a link on the station (to /bkupgrade/data), and the running
/// program's path comes resolved.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The archive at `url`, whole, into memory (the gateway is ~10–30 MB). Over https only, each redirect too
/// (`feed::secure`).
pub async fn download(url: &str) -> Result<Vec<u8>, String> {
    super::feed::secure(url)?;
    let client = super::feed::client_builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| crate::tr!("Загрузка: {error}", error = error))?;
    let response = client.get(url).send().await.map_err(|error| crate::tr!("Загрузка: {error}", error = error))?;
    if !response.status().is_success() {
        return Err(crate::tr!("Загрузка: HTTP {status}", status = response.status()));
    }
    if response.content_length().is_some_and(|length| length as usize > MAX_DOWNLOAD) {
        return Err("download: the file is too large for an update".into());
    }
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|error| crate::tr!("Загрузка: {error}", error = error))?
    {
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
    let program = find(archive, name, u64::MAX)?.ok_or_else(|| crate::tr!("В архиве нет {name}.", name = name))?;
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
            return Err(crate::tr!("Архив: {name} слишком велик.", name = name));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| crate::tr!("Архив: {error}", error = error))?;
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
    let entries = tar.entries().map_err(|error| crate::tr!("Архив: {error}", error = error))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| crate::tr!("Архив: {error}", error = error))?;
        let path = entry.path().map_err(|error| crate::tr!("Архив: {error}", error = error))?.into_owned();
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
        crate::tr!("В архиве нет {file}: это выпуск до 0.2.1 или собранный не конвейером выпуска.", file = RELEASE_FILE)
    })?;
    let release: Release = serde_json::from_slice(&text)
        .map_err(|error| crate::tr!("{file} не читается: {error}", file = RELEASE_FILE, error = error))?;
    if release.platform != platform {
        return Err(crate::tr!(
            "Архив собран для {built}, а здесь {platform}.",
            built = release.platform,
            platform = platform
        ));
    }
    let (Some(signed), Some(wanted)) = (parse_version(&release.version), parse_version(offered)) else {
        return Err(crate::tr!(
            "Версия не читается: в архиве «{inside}», предложена «{offered}».",
            inside = release.version,
            offered = offered
        ));
    };
    if signed != wanted {
        return Err(crate::tr!(
            "Архив подписан как версия {signed}, а предложен как {wanted}.",
            signed = signed,
            wanted = wanted
        ));
    }
    if !is_newer(&release.version, current) {
        return Err(crate::tr!(
            "Версия {signed} не новее стоящей {current}: откат на прежнюю версию не ставится.",
            signed = signed,
            current = current
        ));
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
/// half of one. The `.previous` kept from before goes first: the running program is the way back now, and three
/// programs never stand on the disk at once.
pub fn replace(current: &Path, program: &[u8]) -> std::io::Result<PathBuf> {
    let _ = std::fs::remove_file(previous_of(current));
    let incoming = incoming_of(current);
    std::fs::write(&incoming, program)?;
    swap_in(current, &incoming)
}

/// The program named `name` inside a `.tar.gz` put in place of `current` as `replace` puts it, but streamed from the
/// archive to the disk: never whole in memory. A Station Lite 2 has some 90 MB to spare; the update that held the
/// 16 MB program in memory — beside the archive, which came as base64 in JSON — got the station's gateway killed by
/// the kernel midway, twice on 11.11.2026 (0.4.0 and 0.5.0), and it came back as the old version. Nothing is left
/// half-written: a failure removes the incoming file and the running program stays.
///
/// The `.previous` kept from before goes first, as in `replace`: until 16.11.2026 it went only once the new program
/// was written, and an update in the first ten minutes after another needed room for three programs.
pub fn replace_from_archive(current: &Path, archive: &[u8], name: &str) -> Result<PathBuf, String> {
    let _ = std::fs::remove_file(previous_of(current));
    let incoming = incoming_of(current);
    let on_disk = |error: std::io::Error| format!("{}: {error}", incoming.display());
    let written = with_file(archive, name, |entry| {
        let mut file = std::fs::File::create(&incoming).map_err(on_disk)?;
        let written = std::io::copy(entry, &mut file).map_err(|error| crate::tr!("Архив: {error}", error = error))?;
        file.sync_all().map_err(on_disk)?;
        Ok(written)
    });
    let outcome = match written {
        Ok(Some(written)) if written >= 1024 => {
            swap_in(current, &incoming).map_err(|error| format!("{}: {error}", current.display()))
        }
        Ok(Some(_)) => Err("archive: the program in it is empty".to_string()),
        Ok(None) => Err(crate::tr!("В архиве нет {name}.", name = name)),
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

/// The program named `name` inside a `.tar.gz` written to `target` as it streams out of the archive, made executable:
/// its size and SHA-256. A failure leaves nothing at `target`.
pub fn unpack_to(archive: &[u8], name: &str, target: &Path) -> Result<(u64, String), String> {
    let on_disk = |error: std::io::Error| format!("{}: {error}", target.display());
    let written = with_file(archive, name, |entry| {
        let mut file = std::fs::File::create(target).map_err(on_disk)?;
        let (size, sha) = copy_hashed(entry, &mut file).map_err(|error| crate::tr!("Архив: {error}", error = error))?;
        file.sync_all().map_err(on_disk)?;
        Ok((size, sha))
    });
    let outcome = match written {
        Ok(Some((size, sha))) if size >= 1024 => make_executable(target).map(|_| (size, sha)).map_err(on_disk),
        Ok(Some(_)) => Err("archive: the program in it is empty".to_string()),
        Ok(None) => Err(crate::tr!("В архиве нет {name}.", name = name)),
        Err(error) => Err(error),
    };
    if outcome.is_err() {
        let _ = std::fs::remove_file(target);
    }
    outcome
}

/// Why `answers_version` failed: the program could not be run here at all (a /tmp the system will not run programs
/// from), or it ran and was not the version wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAnswered {
    NotRun(String),
    Wrong(String),
}

impl NotAnswered {
    pub fn message(&self) -> &str {
        match self {
            NotAnswered::NotRun(message) | NotAnswered::Wrong(message) => message,
        }
    }
}

/// Whether the program at `program` runs here and is the version wanted: `<program> version` answers with it within
/// `wait`. A program that does not start on this machine, or names another version, stops here — before the gateway
/// becomes it.
pub fn answers_version(program: &Path, version: &str, wait: Duration) -> Result<(), NotAnswered> {
    let mut child = std::process::Command::new(program)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| NotAnswered::NotRun(crate::tr!("новая версия не запускается: {error}", error = error)))?;
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NotAnswered::Wrong(crate::tr!(
                    "новая версия не назвала себя за {seconds} с",
                    seconds = wait.as_secs().max(1)
                )));
            }
        }
    }
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    let wanted = version.trim_start_matches('v');
    if said.split_whitespace().any(|word| word.trim_start_matches('v') == wanted) {
        Ok(())
    } else {
        Err(NotAnswered::Wrong(crate::tr!(
            "новая версия назвалась «{said}», а ждали {wanted}",
            said = said.trim(),
            wanted = wanted
        )))
    }
}

/// How the program from memory took its place on the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moved {
    /// There was no room for both: the old one went before the new one was written — the seconds when a power cut
    /// leaves no program on the disk.
    pub tight: bool,
}

/// The program tried from memory (`source`) put in the place of the one on the disk (`program`), checked against
/// `sha256` there — read back from the disk — before it takes that place. With room for both (`free_kb`, the disk's
/// free room now, and the floor) it is written beside the old one, which goes only by the rename that puts the new
/// one whole in its place. Without, the old one goes first — it does not run now, so its room is free at once — and
/// a power cut in those seconds leaves no program on the disk: the station then boots as the usual Alice (voice.sh)
/// and the computer puts the gateway back. Whatever fails, nothing half-written keeps the program's name.
pub fn move_to_disk(source: &Path, program: &Path, sha256: &str, free_kb: Option<u64>) -> Result<Moved, String> {
    let size = std::fs::metadata(source).map_err(|error| format!("{}: {error}", source.display()))?.len();
    let need_kb = size.div_ceil(1024) + STATION_FLOOR_KB;
    let old_kb = std::fs::metadata(program).map(|meta| meta.len().div_ceil(1024)).unwrap_or(0);
    let free_kb = free_kb.unwrap_or(u64::MAX);
    let tight = free_kb < need_kb;
    if tight && free_kb.saturating_add(old_kb) < need_kb {
        return Err(crate::tr!(
            "На колонке не хватает памяти для новой версии: нужно {need} МБ, свободно {free} МБ вместе со старой.",
            need = need_kb / 1024,
            free = free_kb.saturating_add(old_kb) / 1024
        ));
    }
    let incoming = incoming_of(program);
    let _ = std::fs::remove_file(&incoming);
    if tight {
        match std::fs::remove_file(program) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", program.display())),
        }
    }
    let put = || -> Result<(), String> {
        let on_disk = |error: std::io::Error| format!("{}: {error}", incoming.display());
        let mut from = std::fs::File::open(source).map_err(|error| format!("{}: {error}", source.display()))?;
        let mut to = std::fs::File::create(&incoming).map_err(on_disk)?;
        let (_, written) = copy_hashed(&mut from, &mut to).map_err(on_disk)?;
        to.sync_all().map_err(on_disk)?;
        drop(to);
        let on_flash = sha256_of(&incoming).map_err(on_disk)?;
        if written != sha256 || on_flash != sha256 {
            return Err(crate::tr!("Новая версия записалась на диск с ошибкой: её сумма не та."));
        }
        make_executable(&incoming).map_err(on_disk)?;
        std::fs::rename(&incoming, program).map_err(|error| format!("{}: {error}", program.display()))
    };
    match put() {
        Ok(()) => Ok(Moved { tight }),
        Err(error) => {
            let _ = std::fs::remove_file(&incoming);
            Err(error)
        }
    }
}

/// The SHA-256 of a file, lower-case hex, read as it streams.
pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    Ok(copy_hashed(&mut file, &mut std::io::sink())?.1)
}

/// `from` copied into `to`, hashed on the way: the bytes copied and their SHA-256.
fn copy_hashed(from: &mut impl std::io::Read, to: &mut impl std::io::Write) -> std::io::Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        digest.update(&buffer[..read]);
        to.write_all(&buffer[..read])?;
        total += read as u64;
    }
    to.flush()?;
    Ok((total, hex::encode(digest.finalize())))
}

fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
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

    #[test]
    fn never_three_programs_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(program_name());
        std::fs::write(&current, b"running").unwrap();
        std::fs::write(previous_of(&current), b"kept from before").unwrap();
        let program: Vec<u8> = (0..4096u32).map(|n| (n % 7) as u8).collect();
        // The kept one goes before the new one is written: the running one becomes the way back.
        let archived = archive(&[(program_name(), &program)]);
        let previous = replace_from_archive(&current, &archived, program_name()).unwrap();
        assert_eq!(std::fs::read(&previous).unwrap(), b"running", "the way back is the one that ran");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn knows_the_programs_size_before_writing_it() {
        let program = vec![1u8; 300_000];
        let folder = format!("duowave-0.13.0/{}", program_name());
        assert_eq!(
            program_size(&archive(&[("README.md", b"x"), (&folder, &program)]), program_name()).unwrap(),
            300_000
        );
        assert!(program_size(&archive(&[("other", &program)]), program_name()).is_err());
        assert!(program_size(&archive(&[(program_name(), b"tiny")]), program_name()).is_err());
    }

    #[test]
    fn the_reserve_is_the_growth_and_the_floor() {
        // 20.4 MB now: 15 % is 3 MB, and the floor 5.
        let reserve = update_reserve_kb(20_400 * 1024);
        assert_eq!(reserve, 20_400 * 15 / 100 + STATION_FLOOR_KB);
        assert_eq!(update_reserve_kb(4 * 1024 * 1024), 2 * 1024 + STATION_FLOOR_KB, "never under 2 MB of growth");
    }

    #[test]
    fn leftovers_go_and_the_way_back_stays() {
        let dir = tempfile::tempdir().unwrap();
        // The program by its name on this system: duowave.exe on Windows (its leftovers are named after it).
        let name = program_name();
        let current = dir.path().join(name);
        std::fs::write(&current, b"running").unwrap();
        std::fs::write(dir.path().join(format!("{name}.failed")), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.path().join(format!("{name}.new")), vec![0u8; 200]).unwrap();
        std::fs::write(dir.path().join(format!(".{name}.new")), vec![0u8; 30]).unwrap();
        std::fs::write(previous_of(&current), b"way back").unwrap();
        assert_eq!(remove_leftovers(&current), 1230);
        assert!(previous_of(&current).exists() && current.exists());
        assert_eq!(remove_leftovers(&current), 0, "nothing more");
    }

    #[test]
    fn unpacks_the_program_to_memory_with_its_sum() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("duowave-0.13.0");
        let program: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
        let (size, sha) = unpack_to(&archive(&[(program_name(), &program)]), program_name(), &target).unwrap();
        assert_eq!((size, sha.as_str()), (200_000, hex::encode(Sha256::digest(&program)).as_str()));
        assert_eq!(std::fs::read(&target).unwrap(), program);
        assert!(unpack_to(&archive(&[(program_name(), b"tiny")]), program_name(), &target).is_err());
        assert!(!target.exists(), "nothing left of a failure");
    }

    // Unix only: the program is a shell script answering as the gateway does; the same check on Windows has its own
    // test below, with a real program.
    #[cfg(unix)]
    #[test]
    fn asks_the_new_program_its_version() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("duowave-9.9.9");
        std::fs::write(&program, "#!/bin/sh\necho \"duowave 9.9.9\"\n").unwrap();
        make_executable(&program).unwrap();
        assert!(answers_version(&program, "v9.9.9", Duration::from_secs(10)).is_ok());
        assert!(matches!(answers_version(&program, "9.9.10", Duration::from_secs(10)), Err(NotAnswered::Wrong(_))));
        let silent = dir.path().join("silent");
        std::fs::write(&silent, "#!/bin/sh\nsleep 5\n").unwrap();
        make_executable(&silent).unwrap();
        assert!(matches!(answers_version(&silent, "9.9.9", Duration::from_millis(300)), Err(NotAnswered::Wrong(_))));
        assert!(matches!(
            answers_version(&dir.path().join("none"), "9.9.9", Duration::from_secs(1)),
            Err(NotAnswered::NotRun(_))
        ));
    }

    /// Windows: the version check with a real program (`duowave-test-stub` as `duowave`, its version in its last
    /// bytes); a program that says nothing in time, and one that is not there.
    #[cfg(windows)]
    #[test]
    fn asks_the_new_program_its_version_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("duowave-9.9.9.exe");
        std::fs::write(&program, crate::updates::testing::program_answering("9.9.9")).unwrap();
        assert!(answers_version(&program, "v9.9.9", Duration::from_secs(10)).is_ok());
        assert!(matches!(answers_version(&program, "9.9.10", Duration::from_secs(10)), Err(NotAnswered::Wrong(_))));
        let silent = crate::utils::test_stub::put(dir.path(), "sleep");
        assert!(matches!(answers_version(&silent, "9.9.9", Duration::from_millis(300)), Err(NotAnswered::Wrong(_))));
        assert!(matches!(
            answers_version(&dir.path().join("none.exe"), "9.9.9", Duration::from_secs(1)),
            Err(NotAnswered::NotRun(_))
        ));
    }

    #[test]
    fn moves_the_program_from_memory_onto_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in-memory");
        let program = dir.path().join("duowave");
        let new: Vec<u8> = (0..300_000u32).map(|n| (n % 13) as u8).collect();
        std::fs::write(&source, &new).unwrap();
        let sha = hex::encode(Sha256::digest(&new));
        // Room for both: written beside, the old one goes by the rename.
        std::fs::write(&program, b"old").unwrap();
        assert_eq!(move_to_disk(&source, &program, &sha, Some(100 * 1024)).unwrap(), Moved { tight: false });
        assert_eq!(std::fs::read(&program).unwrap(), new);
        // Room for the new one only once the old one goes: it goes first.
        std::fs::write(&program, vec![0u8; 400_000]).unwrap();
        let free = STATION_FLOOR_KB + 100;
        assert_eq!(move_to_disk(&source, &program, &sha, Some(free)).unwrap(), Moved { tight: true });
        assert_eq!(std::fs::read(&program).unwrap(), new);
        // No room even then: nothing is touched.
        std::fs::write(&program, b"old").unwrap();
        assert!(move_to_disk(&source, &program, &sha, Some(10)).is_err());
        assert_eq!(std::fs::read(&program).unwrap(), b"old");
        // A sum that is not the program's: the old one stays, nothing half-written keeps its name.
        assert!(move_to_disk(&source, &program, "0000", Some(100 * 1024)).is_err());
        assert_eq!(std::fs::read(&program).unwrap(), b"old");
        assert!(!incoming_of(&program).exists());
    }

    #[test]
    fn the_same_file_by_another_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("duowave");
        std::fs::write(&file, b"x").unwrap();
        #[cfg(unix)]
        {
            let link = dir.path().join("data");
            std::os::unix::fs::symlink(dir.path(), &link).unwrap();
            assert!(same_file(&link.join("duowave"), &file), "/data is a link on the station");
        }
        assert!(!same_file(&file, &dir.path().join("other")));
    }
}
