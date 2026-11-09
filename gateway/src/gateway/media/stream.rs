//! The gateway's own way for a stream the station keeps silent on (docs/plans/plugins.md, the owner's decision 9,
//! stage 5а). Some servers the station plays silently even over https (07.11: «Радио Джаз» from main.hostingradio.ru),
//! and it does not play AAC at all («Радио Рекорд»). Through the home network the station plays our /media (checked
//! on music), so the gateway opens the stream itself and gives it on at `/media/stream/<token>`:
//!
//! - an MP3 or an HLS segment stream goes through as it is: tens of KB/s (128–320 kbit/s), a few per cent of a
//!   processor, a buffer under 1 MB — fit for the gateway on the station too;
//! - an AAC stream is transcoded into MP3 by ffmpeg where that is allowed: on a computer, and on the station only by
//!   the owner's flag MEDIA_TRANSCODE_ON_STATION (`SkillsConfig::transcode_on_station`) — decoding AAC and encoding
//!   MP3 in real time loads the station's weak processor and competes with Alice's own voice processing. Off, an AAC
//!   station is not relayed there, and the plugin hears that it was silent.
//!
//! A token stands for one stream link the playback watch chose to relay; the station fetches it from the home
//! network only (the /media rules), unsigned like the rest of /media. Tokens live for a day.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;

use crate::platform::http::ApiError;
use crate::skills::media::playback::StreamRelay;

/// How long a relayed link stays good.
const TOKEN_LIFE: Duration = Duration::from_secs(24 * 3600);
/// How many links are kept at most; the oldest go first.
const MAX_TOKENS: usize = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where ffmpeg is: FFMPEG, the PATH, then the usual places (Homebrew's too: a gateway started by a service may not
/// have it in its PATH).
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FFMPEG").map(PathBuf::from).filter(|path| path.is_file()) {
        return Some(path);
    }
    let from_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join("ffmpeg")).collect::<Vec<_>>())
        .unwrap_or_default();
    let usual = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"].map(PathBuf::from);
    from_path.into_iter().chain(usual).find(|path| path.is_file())
}

/// Whether a stream is AAC, by its type or its link: the station does not play it as it is.
pub fn is_aac(content_type: Option<&str>, url: &str) -> bool {
    let kind = content_type.unwrap_or_default().to_ascii_lowercase();
    if kind.contains("aac") || kind.contains("mp4a") {
        return true;
    }
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    path.ends_with(".aac") || path.ends_with(".aacp") || path.ends_with(".m4a")
}

struct Relayed {
    url: String,
    at: Instant,
}

/// The relayed links, and whether this gateway may transcode.
pub struct StreamRelays {
    tokens: Mutex<HashMap<String, Relayed>>,
    transcoder: Option<PathBuf>,
    /// On a computer, or on the station by the owner's flag; read when a stream is asked for (the placement may
    /// change).
    transcode_here: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl StreamRelays {
    pub fn new(transcoder: Option<PathBuf>, transcode_here: Arc<dyn Fn() -> bool + Send + Sync>) -> Arc<StreamRelays> {
        Arc::new(StreamRelays { tokens: Mutex::new(HashMap::new()), transcoder, transcode_here })
    }

    /// Whether an AAC stream can be transcoded here now.
    pub fn transcodes(&self) -> bool {
        self.transcoder.is_some() && (self.transcode_here)()
    }

    fn tokens(&self) -> std::sync::MutexGuard<'_, HashMap<String, Relayed>> {
        self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A token for a stream link: the same link keeps its token.
    pub fn register(&self, url: &str) -> String {
        let mut tokens = self.tokens();
        tokens.retain(|_, relayed| relayed.at.elapsed() < TOKEN_LIFE);
        if let Some((token, _)) = tokens.iter().find(|(_, relayed)| relayed.url == url) {
            return token.clone();
        }
        if tokens.len() >= MAX_TOKENS {
            if let Some(oldest) = tokens.iter().min_by_key(|(_, relayed)| relayed.at).map(|(token, _)| token.clone()) {
                tokens.remove(&oldest);
            }
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        tokens.insert(token.clone(), Relayed { url: url.to_string(), at: Instant::now() });
        token
    }

    pub fn url_of(&self, token: &str) -> Option<String> {
        self.tokens().get(token).filter(|relayed| relayed.at.elapsed() < TOKEN_LIFE).map(|relayed| relayed.url.clone())
    }

    /// The stream for the station: passed through, or transcoded from AAC where that is allowed.
    pub async fn serve(&self, token: &str) -> Result<Response, ApiError> {
        let url = self.url_of(token).ok_or_else(|| ApiError::not_found("Not Found"))?;
        let upstream = open(&url, false).await?;
        let kind = content_type(&upstream);
        if is_aac(kind.as_deref(), &url) {
            drop(upstream);
            return self.transcoded(&url);
        }
        tracing::info!(target: "media", "relaying a stream for the station ({})", kind.as_deref().unwrap_or("no type"));
        Ok(passed(upstream, kind))
    }

    /// AAC into MP3 by ffmpeg, as it comes; the process goes when the station stops listening.
    fn transcoded(&self, url: &str) -> Result<Response, ApiError> {
        let Some(ffmpeg) = self.transcoder.as_ref().filter(|_| (self.transcode_here)()) else {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "AAC здесь не перекодировать: на колонке это большая нагрузка (MEDIA_TRANSCODE_ON_STATION), или нет ffmpeg",
            ));
        };
        let child = transcoder(ffmpeg, url).map_err(|error| ApiError::internal(format!("ffmpeg: {error}")))?;
        tracing::info!(target: "media", "transcoding an AAC stream into MP3 for the station");
        let body = Body::from_stream(TranscodedStream::new(child));
        let mut response = body.into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }
}

/// An app's own sound on localhost (its `/stream/…`, docs/plans/plugins.md, «Звук приложения на колонку»), given on
/// to the station as it comes: the app chose its format, nothing is transcoded.
pub async fn app_sound(url: &str) -> Result<Response, ApiError> {
    let upstream = open(url, true).await?;
    let kind = content_type(&upstream);
    Ok(passed(upstream, kind))
}

/// A stream opened for the station; `local` for the machine's own address (no proxy).
async fn open(url: &str, local: bool) -> Result<reqwest::Response, ApiError> {
    let mut builder = crate::utils::http::client_builder().connect_timeout(CONNECT_TIMEOUT);
    if local {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|error| ApiError::internal(format!("stream relay: {error}")))?;
    let upstream = client
        .get(url)
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("поток не открылся: {}", error.without_url())))?;
    if !upstream.status().is_success() {
        return Err(ApiError::bad_gateway(format!("поток ответил HTTP {}", upstream.status())));
    }
    Ok(upstream)
}

fn content_type(upstream: &reqwest::Response) -> Option<String> {
    upstream.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(str::to_string)
}

/// The stream as it comes, with a type the station's player knows (audio, an HLS list, Ogg).
fn passed(upstream: reqwest::Response, kind: Option<String>) -> Response {
    let body = Body::from_stream(upstream.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other)));
    let mut response = body.into_response();
    let headers = response.headers_mut();
    let kind = kind
        .filter(|kind| kind.starts_with("audio/") || kind.contains("mpegurl") || kind.starts_with("application/ogg"))
        .unwrap_or_else(|| "audio/mpeg".into());
    if let Ok(value) = HeaderValue::from_str(&kind) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn transcoder(ffmpeg: &Path, url: &str) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-reconnect", "1", "-reconnect_streamed", "1"])
        .args(["-i", url, "-vn", "-c:a", "libmp3lame", "-b:a", "128k", "-f", "mp3", "pipe:1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// ffmpeg's output as a body: the process lives as long as the body, and is killed with it.
struct TranscodedStream {
    _child: tokio::process::Child,
    stdout: Option<tokio_util::io::ReaderStream<tokio::process::ChildStdout>>,
}

impl TranscodedStream {
    fn new(mut child: tokio::process::Child) -> TranscodedStream {
        let stdout = child.stdout.take().map(tokio_util::io::ReaderStream::new);
        TranscodedStream { _child: child, stdout }
    }
}

impl futures::Stream for TranscodedStream {
    type Item = std::io::Result<bytes::Bytes>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.stdout.as_mut() {
            Some(stdout) => std::pin::Pin::new(stdout).poll_next(context),
            None => std::task::Poll::Ready(None),
        }
    }
}

/// The playback watch's way to relay: a token, and the link the station fetches it by.
pub struct Relay(pub Arc<StreamRelays>);

impl StreamRelay for Relay {
    fn relay(&self, url: &str, media_base: Option<&str>) -> Result<String, String> {
        let Some(base) = media_base else {
            return Err("неизвестно, по какому адресу колонка ходит в /media".into());
        };
        if is_aac(None, url) && !self.0.transcodes() {
            return Err("AAC здесь не перекодировать (MEDIA_TRANSCODE_ON_STATION или нет ffmpeg)".into());
        }
        let token = self.0.register(url);
        Ok(format!("{}/stream/{token}", base.trim_end_matches('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_aac_by_its_type_or_its_link_and_keeps_one_token_per_link() {
        assert!(is_aac(Some("audio/aacp"), "https://radio.example/stream"));
        assert!(is_aac(None, "https://radiorecord.hostingradio.ru/rr_main96.aacp"));
        assert!(!is_aac(Some("audio/mpeg"), "https://hitfm.hostingradio.ru/hitfm128.mp3"));
        let relays = StreamRelays::new(None, Arc::new(|| true));
        let relay = Relay(relays.clone());
        let jazz = relay.relay("https://main.hostingradio.ru/jazz-128.mp3", Some("http://192.0.2.10/media")).unwrap();
        assert!(jazz.starts_with("http://192.0.2.10/media/stream/"), "{jazz}");
        let token = jazz.rsplit('/').next().unwrap();
        assert_eq!(relays.url_of(token).as_deref(), Some("https://main.hostingradio.ru/jazz-128.mp3"));
        assert_eq!(
            relay.relay("https://main.hostingradio.ru/jazz-128.mp3", Some("http://192.0.2.10/media")).unwrap(),
            jazz
        );
        // No ffmpeg: an AAC station is not relayed.
        assert!(relay.relay("https://record.example/rr.aacp", Some("http://192.0.2.10/media")).is_err());
        assert!(relay.relay("https://x.example/a.mp3", None).is_err());
        let station = StreamRelays::new(Some(PathBuf::from("/usr/bin/ffmpeg")), Arc::new(|| false));
        assert!(!station.transcodes(), "on the station without the owner's flag");
    }
}
