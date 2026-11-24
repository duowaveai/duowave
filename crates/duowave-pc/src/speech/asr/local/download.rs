//! A model file fetched whole or not at all. It comes into `<file>.part`, is resumed after a break (HTTP Range), is
//! checked by its size and SHA-256, and only then takes its own name — a file under its name is always a whole one.
//! A file that does not match is deleted, never loaded. Addresses are tried in order (a mirror, then Hugging Face).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// What the file must be.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub size: u64,
    pub sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DownloadError {
    #[error("остановлено")]
    Cancelled,
    #[error("на диске свободно {free_mb} МБ, а нужно ещё {need_mb} МБ")]
    NoSpace { need_mb: u64, free_mb: u64 },
    #[error("{0}")]
    Failed(String),
}

/// No byte for this long: the connection is taken for dead and resumed.
const IDLE: Duration = Duration::from_secs(60);
/// Tries per address: a break resumes where it stopped.
const TRIES: u32 = 4;
/// Room left on the disk beside the file.
const SPARE_BYTES: u64 = 200 * 1024 * 1024;

/// `<file>.part` beside the file.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(".part");
    target.with_file_name(name)
}

/// The file whole under its name: present with its size (only a checked file is ever renamed to it).
pub fn present(target: &Path, size: u64) -> bool {
    std::fs::metadata(target).is_ok_and(|meta| meta.is_file() && meta.len() == size)
}

/// Fetches the file to `target`. `progress(bytes, total)` is told as bytes come, the resumed part included.
pub async fn fetch(
    urls: &[String],
    target: &Path,
    expected: Expected<'_>,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    cancel: &CancellationToken,
) -> Result<(), DownloadError> {
    if present(target, expected.size) {
        progress(expected.size, expected.size);
        return Ok(());
    }
    let dir = target.parent().ok_or_else(|| DownloadError::Failed("нет папки для модели".into()))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|error| DownloadError::Failed(format!("{}: {error}", dir.display())))?;
    let part = part_path(target);
    let have = tokio::fs::metadata(&part).await.map(|meta| meta.len()).unwrap_or(0);
    let need = expected.size.saturating_sub(have).saturating_add(SPARE_BYTES);
    if let Some(free) = free_bytes(dir) {
        if free < need {
            return Err(DownloadError::NoSpace { need_mb: need / 1_000_000, free_mb: free / 1_000_000 });
        }
    }
    let mut last = String::from("нет адреса");
    for url in urls {
        for attempt in 0..TRIES {
            if cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            match attempt_once(url, &part, expected, progress, cancel).await {
                Ok(()) => {
                    tokio::fs::rename(&part, target)
                        .await
                        .map_err(|error| DownloadError::Failed(format!("{}: {error}", target.display())))?;
                    return Ok(());
                }
                Err(Attempt::Cancelled) => return Err(DownloadError::Cancelled),
                Err(Attempt::Corrupt(why)) => {
                    // A whole file with the wrong bytes: from the start, once more, then the next address.
                    let _ = tokio::fs::remove_file(&part).await;
                    last = why;
                }
                Err(Attempt::Again(why)) => last = why,
                Err(Attempt::NextAddress(why)) => {
                    last = why;
                    break;
                }
            }
            let pause = Duration::from_secs(2u64.pow(attempt.min(4)));
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            }
        }
    }
    Err(DownloadError::Failed(format!("модель не скачалась: {last}")))
}

enum Attempt {
    Cancelled,
    /// A break or a server's hiccup: the same address again, from where it stopped.
    Again(String),
    /// The whole file came with other bytes.
    Corrupt(String),
    /// This address will not give the file (404, 403…).
    NextAddress(String),
}

async fn attempt_once(
    url: &str,
    part: &Path,
    expected: Expected<'_>,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    cancel: &CancellationToken,
) -> Result<(), Attempt> {
    let io = |error: std::io::Error| Attempt::Again(format!("{}: {error}", part.display()));
    // The part's bytes so far go into the hash first: a resume checks the whole file.
    let (mut hasher, mut have) = hash_part(part).await.map_err(io)?;
    if have > expected.size {
        let _ = tokio::fs::remove_file(part).await;
        (hasher, have) = (Sha256::new(), 0);
    }
    if have == expected.size {
        return finish(hasher, expected);
    }
    let mut request = crate::utils::http::client().get(url).timeout(Duration::from_secs(24 * 3600));
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let response = tokio::select! {
        response = request.send() => response.map_err(|error| Attempt::Again(innermost(&error)))?,
        _ = cancel.cancelled() => return Err(Attempt::Cancelled),
    };
    let status = response.status();
    let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0 {
        tokio::fs::OpenOptions::new().append(true).open(part).await.map_err(io)?
    } else if status.is_success() {
        // The server sent the whole file (it ignores Range): from the start.
        (hasher, have) = (Sha256::new(), 0);
        tokio::fs::File::create(part).await.map_err(io)?
    } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = tokio::fs::remove_file(part).await;
        return Err(Attempt::Again(format!("HTTP {}", status.as_u16())));
    } else if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Attempt::Again(format!("HTTP {}", status.as_u16())));
    } else {
        return Err(Attempt::NextAddress(format!("HTTP {} — {url}", status.as_u16())));
    };
    let mut response = response;
    progress(have, expected.size);
    loop {
        let chunk = tokio::select! {
            chunk = tokio::time::timeout(IDLE, response.chunk()) => chunk,
            _ = cancel.cancelled() => {
                let _ = file.flush().await;
                return Err(Attempt::Cancelled);
            }
        };
        let chunk = match chunk {
            Err(_) => return Err(Attempt::Again("соединение замолчало".into())),
            Ok(Err(error)) => return Err(Attempt::Again(innermost(&error))),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        if have + chunk.len() as u64 > expected.size {
            drop(file);
            let _ = tokio::fs::remove_file(part).await;
            return Err(Attempt::Corrupt("файл больше, чем должен быть".into()));
        }
        file.write_all(&chunk).await.map_err(io)?;
        hasher.update(&chunk);
        have += chunk.len() as u64;
        progress(have, expected.size);
    }
    file.flush().await.map_err(io)?;
    file.sync_all().await.map_err(io)?;
    if have < expected.size {
        return Err(Attempt::Again(format!("пришло {have} из {} байт", expected.size)));
    }
    finish(hasher, expected)
}

fn finish(hasher: Sha256, expected: Expected<'_>) -> Result<(), Attempt> {
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if hex.eq_ignore_ascii_case(expected.sha256) {
        Ok(())
    } else {
        Err(Attempt::Corrupt("файл пришёл с другим содержимым (SHA-256 не совпал)".into()))
    }
}

/// The part's bytes so far through SHA-256; nothing when there is no part.
async fn hash_part(part: &Path) -> std::io::Result<(Sha256, u64)> {
    let mut hasher = Sha256::new();
    let mut file = match tokio::fs::File::open(part).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((hasher, 0)),
        Err(error) => return Err(error),
    };
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((hasher, total))
}

/// Free bytes on the disk of a folder; None when the system does not say.
fn free_bytes(dir: &Path) -> Option<u64> {
    let free = crate::voice_lab::platform::free_disk_bytes(dir);
    (free > 0).then_some(free)
}

fn innermost(error: &(dyn std::error::Error + 'static)) -> String {
    let mut inner = error;
    while let Some(source) = inner.source() {
        inner = source;
    }
    inner.to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;

    use super::*;

    #[derive(Clone)]
    struct Served {
        body: Arc<Vec<u8>>,
        /// Range requests honoured; false: the whole file every time.
        ranges: bool,
        /// Bytes sent before the connection «breaks», on the first request only.
        cut_first_at: Option<usize>,
        requests: Arc<AtomicUsize>,
        ranges_seen: Arc<Mutex<Vec<String>>>,
    }

    async fn file(State(served): State<Served>, headers: HeaderMap) -> impl IntoResponse {
        let number = served.requests.fetch_add(1, Ordering::SeqCst);
        let range = headers.get("range").and_then(|value| value.to_str().ok()).map(str::to_string);
        if let Some(range) = &range {
            served.ranges_seen.lock().unwrap().push(range.clone());
        }
        let start = match (&range, served.ranges) {
            (Some(range), true) => range.trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap_or(0),
            _ => 0usize,
        };
        let mut body = served.body[start..].to_vec();
        if number == 0 {
            if let Some(cut) = served.cut_first_at {
                body.truncate(cut);
            }
        }
        let status = if start > 0 { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
        (status, body)
    }

    async fn serve(served: Served) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route("/model.gguf", get(file)).with_state(served);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{address}/model.gguf")
    }

    fn served(body: Vec<u8>) -> Served {
        Served {
            body: Arc::new(body),
            ranges: true,
            cut_first_at: None,
            requests: Arc::new(AtomicUsize::new(0)),
            ranges_seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn sha(body: &[u8]) -> String {
        Sha256::digest(body).iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|n| (n % 251) as u8).collect()
    }

    #[tokio::test]
    async fn a_whole_file_takes_its_name_only_after_its_hash_and_a_present_one_is_not_fetched_again() {
        let body = body();
        let digest = sha(&body);
        let server = served(body.clone());
        let url = serve(server.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("models").join("model.gguf");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let expected = Expected { size: body.len() as u64, sha256: &digest };
        fetch(
            std::slice::from_ref(&url),
            &target,
            expected,
            &move |bytes, total| log.lock().unwrap().push((bytes, total)),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        assert!(!part_path(&target).exists());
        assert_eq!(seen.lock().unwrap().last(), Some(&(body.len() as u64, body.len() as u64)));
        fetch(&[url], &target, expected, &|_, _| {}, &CancellationToken::new()).await.unwrap();
        assert_eq!(server.requests.load(Ordering::SeqCst), 1, "a present file is not fetched again");
    }

    #[tokio::test]
    async fn a_break_resumes_where_it_stopped_with_a_range_and_the_whole_file_is_checked() {
        let body = body();
        let digest = sha(&body);
        let mut server = served(body.clone());
        server.cut_first_at = Some(100_000);
        let url = serve(server.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("model.gguf");
        let expected = Expected { size: body.len() as u64, sha256: &digest };
        fetch(&[url], &target, expected, &|_, _| {}, &CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        assert_eq!(server.ranges_seen.lock().unwrap().as_slice(), ["bytes=100000-"]);
    }

    #[tokio::test]
    async fn a_server_that_ignores_range_gives_the_whole_file_again_and_it_still_comes_whole() {
        let body = body();
        let digest = sha(&body);
        let mut server = served(body.clone());
        server.ranges = false;
        let url = serve(server).await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("model.gguf");
        std::fs::write(part_path(&target), &body[..50_000]).unwrap();
        let expected = Expected { size: body.len() as u64, sha256: &digest };
        fetch(&[url], &target, expected, &|_, _| {}, &CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
    }

    #[tokio::test]
    async fn other_bytes_are_never_kept_under_the_name_and_a_missing_address_gives_way_to_the_next() {
        let body = body();
        let url = serve(served(body.clone())).await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("model.gguf");
        let wrong = "0".repeat(64);
        let expected = Expected { size: body.len() as u64, sha256: &wrong };
        let missing = url.replace("model.gguf", "nothing.gguf");
        let cancel = CancellationToken::new();
        // The wrong hash makes every try fail: stop it after the first round to keep the test short.
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            stopper.cancel();
        });
        let result = fetch(&[missing, url], &target, expected, &|_, _| {}, &cancel).await;
        assert!(matches!(result, Err(DownloadError::Cancelled | DownloadError::Failed(_))), "{result:?}");
        assert!(!target.exists());
    }
}
