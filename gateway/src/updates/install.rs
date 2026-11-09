//! Installing an update of the gateway alone (a PC, a server, the station): the archive is downloaded into the data
//! folder, its signature checked, and the program inside it replaces the running one by a rename. The program before
//! stays beside it as `duowave.previous` — the way back — and the gateway starts again as the new program, in the same
//! process where the system allows it (exec), so a service manager sees no exit.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An update larger than this is not one of ours.
pub const MAX_DOWNLOAD: usize = 96 * 1024 * 1024;

/// The archive at `url`, whole, into memory (the gateway is ~10–30 MB).
pub async fn download(url: &str) -> Result<Vec<u8>, String> {
    let client = crate::utils::http::client_builder()
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
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = tar.entries().map_err(|error| format!("архив: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("архив: {error}"))?;
        let path = entry.path().map_err(|error| format!("архив: {error}"))?.into_owned();
        let file = path.file_name().and_then(|file| file.to_str()).unwrap_or("");
        if file == name && path.components().count() <= 2 && entry.header().entry_type().is_file() {
            let mut program = Vec::new();
            entry.read_to_end(&mut program).map_err(|error| format!("архив: {error}"))?;
            if program.len() < 1024 {
                return Err("archive: the program in it is empty".into());
            }
            return Ok(program);
        }
    }
    Err(format!("в архиве нет {name}"))
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
    let incoming = current.with_file_name(format!("{}.new", program_name()));
    std::fs::write(&incoming, program)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o755))?;
    }
    let previous = previous_of(current);
    let _ = std::fs::remove_file(&previous);
    // A running program may be renamed (Windows too), not overwritten.
    std::fs::rename(current, &previous)?;
    if let Err(error) = std::fs::rename(&incoming, current) {
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
}
