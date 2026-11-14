//! The voice models on disk, as the voice server scans them (tts-server/server.py): tts-server/models/<card id>/
//! holds a Piper `.onnx` with its `.onnx.json`, or one Silero `.pt`. Everything that touches these files is here,
//! and every path is checked to stay inside the folder: card ids and file names come from a remote registry and
//! from uploads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Where downloads may come from: the showcase's and the open registry's hosts, over HTTPS.
pub const ALLOWED_HOSTS: &[&str] = &["huggingface.co", "models.silero.ai"];

/// The largest file we take: the biggest Piper voices are ~120 MB, Silero ~100 MB.
pub const MAX_MODEL_BYTES: u64 = 1_500_000_000;
pub const MAX_UPLOAD_BYTES: u64 = 500_000_000;

/// How long a file sent by hand may take, as Node's default requestTimeout bounded the request: a sender that
/// vanished mid-file (a phone that left the Wi-Fi) leaves no half file and no busy Silero slot behind.
pub const UPLOAD_DEADLINE: Duration = Duration::from_secs(300);

/// A card id or a file name: letters, digits, dot, dash, underscore; never `..`, never a path.
pub fn safe_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    bytes.len() <= 128
        && first.is_ascii_alphanumeric()
        && rest.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.contains("..")
}

/// A URL of the catalog's hosts, over https.
pub fn allowed_url(url: Option<&str>) -> bool {
    let Some(url) = url else {
        return false;
    };
    match url::Url::parse(url) {
        Ok(parsed) => parsed.scheme() == "https" && parsed.host_str().is_some_and(|host| ALLOWED_HOSTS.contains(&host)),
        Err(_) => false,
    }
}

/// How far a download is: bytes so far, and the whole when the server said it.
#[derive(Debug, Default)]
pub struct Progress {
    pub bytes: AtomicU64,
    pub total: AtomicU64,
}

impl Progress {
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// Whether a download may come from a URL.
pub type UrlCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub struct ModelFiles {
    root: PathBuf,
    /// Where downloads may come from: the showcase's and the registry's hosts (a test serves its own).
    allowed: UrlCheck,
}

impl ModelFiles {
    pub fn new(root: impl AsRef<Path>) -> ModelFiles {
        ModelFiles::with_check(root, Arc::new(|url: &str| allowed_url(Some(url))))
    }

    pub fn with_check(root: impl AsRef<Path>, allowed: UrlCheck) -> ModelFiles {
        let root = root.as_ref();
        ModelFiles { root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()), allowed }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether a model's URL is one this folder downloads from.
    pub fn allows(&self, url: Option<&str>) -> bool {
        url.is_some_and(|url| (self.allowed)(url))
    }

    /// The folder of a card, checked to be inside the models folder.
    pub fn folder(&self, id: &str) -> Result<PathBuf, String> {
        if !safe_name(id) {
            return Err(format!("not a model id: {id}"));
        }
        let folder = self.root.join(id);
        if !folder.starts_with(&self.root) || folder == self.root {
            return Err(format!("outside the models folder: {id}"));
        }
        Ok(folder)
    }

    pub fn path(&self, id: &str, file: &str) -> Result<PathBuf, String> {
        if !safe_name(file) {
            return Err(format!("not a model file: {file}"));
        }
        Ok(self.folder(id)?.join(file))
    }

    /// Installed: the model file is there and, for Piper, its config beside it — what the voice server needs.
    pub fn installed(&self, id: &str, file: Option<&str>) -> bool {
        let Some(file) = file else {
            return false;
        };
        if !safe_name(id) || !safe_name(file) {
            return false;
        }
        match self.path(id, file) {
            Ok(path) => path.exists() && (!file.ends_with(".onnx") || with_suffix(&path, ".json").exists()),
            Err(_) => false,
        }
    }

    /// A model from its URL: every file to `.part` first, renamed only when all of them are whole — the config
    /// before the model, so a model file on disk always has its config. A cancel or a failure leaves nothing.
    pub async fn download(
        &self,
        id: &str,
        file: &str,
        url: &str,
        progress: &Progress,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        if !(self.allowed)(url) {
            return Err(format!("downloads come from {} over https only", ALLOWED_HOSTS.join(", ")));
        }
        let target = self.path(id, file)?;
        let folder = self.folder(id)?;
        std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let part = with_suffix(&target, ".part");
        let json_part = with_suffix(&target, ".json.part");
        // Whatever ends the download — done, failed, canceled, dropped — no half file stays.
        let _cleanup = Cleanup { parts: vec![part.clone(), json_part.clone()], folder };
        let mut whole = Vec::new();
        if file.ends_with(".onnx") {
            pull(&format!("{url}.json"), &json_part, None, cancel).await?;
            whole.push((json_part.clone(), with_suffix(&target, ".json")));
        }
        pull(url, &part, Some(progress), cancel).await?;
        whole.push((part, target));
        for (part, whole) in &whole {
            std::fs::rename(part, whole).map_err(|error| format!("{}: {error}", whole.display()))?;
        }
        Ok(())
    }

    /// A model's files gone, and its folder when nothing else is in it.
    pub fn remove(&self, id: &str, file: &str) -> Result<(), String> {
        let target = self.path(id, file)?;
        remove_quietly(&target).map_err(|error| format!("{}: {error}", target.display()))?;
        remove_quietly(&with_suffix(&target, ".json")).map_err(|error| format!("{}: {error}", target.display()))?;
        remove_if_empty(&self.folder(id)?);
        Ok(())
    }

    /// A Silero file sent by hand, into the card's folder: streamed to `.part`, at most MAX_UPLOAD_BYTES, within
    /// UPLOAD_DEADLINE.
    pub async fn place<S, E>(&self, id: &str, file: &str, body: S) -> Result<(), String>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        if !file.ends_with(".pt") {
            return Err("a Silero model is a .pt file".into());
        }
        let target = self.path(id, file)?;
        let folder = self.folder(id)?;
        std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let part = with_suffix(&target, ".part");
        let _cleanup = Cleanup { parts: vec![part.clone()], folder };
        tokio::time::timeout(UPLOAD_DEADLINE, write_limited(body, &part, None, MAX_UPLOAD_BYTES, None))
            .await
            .map_err(|_| crate::tr!("не пришёл целиком за {seconds} с", seconds = UPLOAD_DEADLINE.as_secs()))??;
        std::fs::rename(&part, &target).map_err(|error| format!("{}: {error}", target.display()))?;
        Ok(())
    }

    /// What a crash or a restart mid-download left: half files, never loaded, only taking space.
    pub fn cleanup(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut folders: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|entry| std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()))
            .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
            .collect();
        folders.sort();
        let mut removed = Vec::new();
        for (name, folder) in folders {
            let Ok(files) = std::fs::read_dir(&folder) else {
                continue;
            };
            let mut halves: Vec<String> = files
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|file| file.ends_with(".part"))
                .collect();
            halves.sort();
            for half in halves {
                if remove_quietly(&folder.join(&half)).is_ok() {
                    removed.push(format!("{name}/{half}"));
                }
            }
            remove_if_empty(&folder);
        }
        removed
    }
}

/// `path` with `suffix` after its whole name: `x.onnx` → `x.onnx.json`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// A file gone; one that was not there is fine.
fn remove_quietly(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn remove_if_empty(folder: &Path) {
    // Already gone, or not readable: nothing to do.
    if let Ok(mut entries) = std::fs::read_dir(folder) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// The half files of a download or an upload, removed however it ends, and the card's folder when it is empty.
struct Cleanup {
    parts: Vec<PathBuf>,
    folder: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for part in &self.parts {
            let _ = remove_quietly(part);
        }
        remove_if_empty(&self.folder);
    }
}

/// The last part of a URL: what an error names.
fn file_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

fn megabytes(bytes: u64) -> u64 {
    (bytes as f64 / 1_048_576.0).round() as u64
}

async fn pull(url: &str, file: &Path, progress: Option<&Progress>, cancel: &CancellationToken) -> Result<(), String> {
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err("canceled".into()),
        response = crate::utils::http::client().get(url).send() => response.map_err(|error| error.to_string())?,
    };
    if !response.status().is_success() {
        return Err(format!("HTTP {} for {}", response.status().as_u16(), file_of(url)));
    }
    // Redirects end on the hosts' CDNs: the first hop is what we check, the size is what we bound.
    let total = response.content_length().unwrap_or(0);
    if total > MAX_MODEL_BYTES {
        return Err(crate::tr!("{file}: {size} МБ — больше предела", file = file_of(url), size = megabytes(total)));
    }
    if let Some(progress) = progress {
        progress.total.store(total, Ordering::Relaxed);
    }
    write_limited(response.bytes_stream(), file, progress, MAX_MODEL_BYTES, Some(cancel)).await
}

/// Streams a body into a file, failing past `max` bytes: a download or an upload never fills the disk.
async fn write_limited<S, E>(
    mut body: S,
    file: &Path,
    progress: Option<&Progress>,
    max: u64,
    cancel: Option<&CancellationToken>,
) -> Result<(), String>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let out = tokio::fs::File::create(file).await.map_err(|error| format!("{}: {error}", file.display()))?;
    let mut out = tokio::io::BufWriter::with_capacity(1 << 20, out);
    let mut seen: u64 = 0;
    loop {
        let next = match cancel {
            Some(cancel) => tokio::select! {
                _ = cancel.cancelled() => return Err("canceled".into()),
                next = body.next() => next,
            },
            None => body.next().await,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| error.to_string())?;
        seen += chunk.len() as u64;
        if let Some(progress) = progress {
            progress.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        if seen > max {
            return Err(crate::tr!("больше {size} МБ", size = megabytes(max)));
        }
        out.write_all(&chunk).await.map_err(|error| format!("{}: {error}", file.display()))?;
    }
    out.flush().await.map_err(|error| format!("{}: {error}", file.display()))?;
    out.into_inner().sync_all().await.map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use axum::routing::get;
    use axum::Router;

    /// A host for the files: a voice's model, and its config, which comes after `json_delay`.
    async fn serve(json_delay: Duration) -> String {
        let router = Router::new().route(
            "/{name}",
            get(move |axum::extract::Path(name): axum::extract::Path<String>| async move {
                if name.ends_with(".onnx.json") {
                    tokio::time::sleep(json_delay).await;
                    return (axum::http::StatusCode::OK, Bytes::from_static(br#"{"audio": {}}"#));
                }
                if name.ends_with(".onnx") {
                    return (axum::http::StatusCode::OK, Bytes::from(vec![7u8; 64 * 1024]));
                }
                (axum::http::StatusCode::NOT_FOUND, Bytes::new())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    fn files(root: &Path, base: &str) -> ModelFiles {
        let base = base.to_string();
        ModelFiles::with_check(root, Arc::new(move |url: &str| url.starts_with(&base)))
    }

    #[test]
    fn takes_only_plain_names_and_the_hosts_of_the_catalog_over_https() {
        assert!(safe_name("ru_RU-denis-medium.onnx"));
        assert!(!safe_name("../evil.pt"));
        assert!(!safe_name("a/b"));
        assert!(!safe_name(".."));
        assert!(!safe_name(""));
        assert!(!safe_name(&"a".repeat(129)));
        assert!(allowed_url(Some("https://huggingface.co/rhasspy/x.onnx")));
        assert!(!allowed_url(Some("http://huggingface.co/x.onnx")));
        assert!(!allowed_url(Some("https://evil.example/x.onnx")));
        assert!(!allowed_url(None));
        let root = tempfile::tempdir().unwrap();
        assert!(ModelFiles::new(root.path()).folder("../outside").is_err());
    }

    #[tokio::test]
    async fn installs_a_piper_voice_whole_the_model_and_its_config_never_one_without_the_other() {
        let root = tempfile::tempdir().unwrap();
        let base = serve(Duration::ZERO).await;
        let progress = Progress::default();
        let files = files(root.path(), &base);
        files
            .download("piper-x", "x.onnx", &format!("{base}/x.onnx"), &progress, &CancellationToken::new())
            .await
            .unwrap();
        assert!(files.installed("piper-x", Some("x.onnx")));
        assert_eq!(progress.bytes(), 64 * 1024);
        assert_eq!(progress.total(), 64 * 1024);
        let config = std::fs::read_to_string(root.path().join("piper-x/x.onnx.json")).unwrap();
        assert!(config.contains("audio"));
    }

    #[tokio::test]
    async fn leaves_nothing_behind_when_a_download_is_canceled() {
        let root = tempfile::tempdir().unwrap();
        let base = serve(Duration::from_secs(2)).await;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop.cancel();
        });
        let result = files(root.path(), &base)
            .download("piper-y", "y.onnx", &format!("{base}/y.onnx"), &Progress::default(), &cancel)
            .await;
        assert!(result.is_err());
        assert!(!root.path().join("piper-y").exists());
    }

    #[tokio::test]
    async fn refuses_what_does_not_come_from_the_catalog_hosts() {
        let root = tempfile::tempdir().unwrap();
        let base = serve(Duration::ZERO).await;
        let error = ModelFiles::new(root.path())
            .download("piper-z", "z.onnx", &format!("{base}/z.onnx"), &Progress::default(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("https"), "{error}");
        assert!(!root.path().join("piper-z").exists());
    }

    #[test]
    fn removes_a_silero_model_without_a_config_of_its_own_and_its_folder() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("silero")).unwrap();
        std::fs::write(root.path().join("silero/v4_ru.pt"), "x").unwrap();
        let files = ModelFiles::new(root.path());
        assert!(files.installed("silero", Some("v4_ru.pt")));
        files.remove("silero", "v4_ru.pt").unwrap();
        assert!(!root.path().join("silero").exists());
    }

    #[tokio::test]
    async fn takes_a_silero_file_by_hand_only_as_a_pt_and_clears_the_halves_a_crash_left() {
        let root = tempfile::tempdir().unwrap();
        let files = ModelFiles::new(root.path());
        let body = || futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"torch"))]);
        files.place("silero", "v4_ru.pt", body()).await.unwrap();
        assert!(files.installed("silero", Some("v4_ru.pt")));
        let error = files.place("silero", "evil.sh", body()).await.unwrap_err();
        assert!(error.contains(".pt"), "{error}");
        std::fs::create_dir_all(root.path().join("piper-half")).unwrap();
        std::fs::write(root.path().join("piper-half/h.onnx.part"), "x").unwrap();
        assert_eq!(files.cleanup(), vec!["piper-half/h.onnx.part"]);
        assert!(!root.path().join("piper-half").exists());
    }
}
