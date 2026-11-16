//! The control panel built in panel/ (Vite + Preact, `pnpm --filter panel build`), served at /panel. gateway/build.rs
//! packs panel/dist into the program with its scripts, styles and pages gzip-compressed — the station keeps the program
//! on a small flash — and a browser gets them as they are kept (`Content-Encoding: gzip`); a client that takes no gzip
//! gets them inflated. A debug build reads panel/dist from the disk instead, as rust-embed does there, so a panel built
//! again shows at the next reload. Not built — build.rs leaves the folder empty — the route says so instead of failing
//! the gateway.

use std::borrow::Cow;
use std::io::Read as _;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::platform::http::ApiError;

/// A debug build's panel: panel/dist as it is on the disk now (in a debug build rust-embed reads the folder at each
/// request and embeds nothing).
#[cfg(debug_assertions)]
#[derive(rust_embed::RustEmbed)]
#[folder = "../panel/dist"]
struct OnDisk;

/// The panel as gateway/build.rs packed it into the program (a debug build carries it for its tests only).
#[cfg(any(test, not(debug_assertions)))]
mod packed {
    use std::borrow::Cow;

    use super::PanelFile;

    /// Where a packed file lies in `BLOB`, and whether it is kept gzip-compressed.
    pub struct Packed {
        pub path: &'static str,
        pub gzip: bool,
        pub start: usize,
        pub end: usize,
    }

    include!(concat!(env!("OUT_DIR"), "/panel_files.rs"));

    /// A packed file by its path (`\` read as `/`, as rust-embed reads it).
    pub fn get(path: &str) -> Option<PanelFile> {
        let path = path.replace('\\', "/");
        let file = &FILES[FILES.binary_search_by(|file| file.path.cmp(path.as_str())).ok()?];
        Some(PanelFile { bytes: Cow::Borrowed(&BLOB[file.start..file.end]), gzip: file.gzip })
    }
}

/// One of the panel's files as it is kept: gzip-compressed, as build.rs keeps text, or the file itself.
#[derive(Clone)]
pub struct PanelFile {
    pub bytes: Cow<'static, [u8]>,
    pub gzip: bool,
}

impl PanelFile {
    /// The file itself: inflated when it is kept compressed.
    fn plain(&self) -> Result<Cow<'_, [u8]>, ApiError> {
        if !self.gzip {
            return Ok(Cow::Borrowed(&self.bytes));
        }
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(&self.bytes[..])
            .read_to_end(&mut plain)
            .map_err(|error| ApiError::internal(format!("a packed panel file does not inflate: {error}")))?;
        Ok(Cow::Owned(plain))
    }

    /// The answer with the file: the gzip as it is kept to a client that takes gzip, the file itself to one that does
    /// not. A file kept compressed answers `Vary: Accept-Encoding` either way, so a cache in between keeps the two apart.
    fn answer(self, request: &HeaderMap) -> Result<Response, ApiError> {
        if !self.gzip {
            return Ok(Response::new(Body::from(self.bytes)));
        }
        let mut response = if takes_gzip(request) {
            let mut response = Response::new(Body::from(self.bytes));
            response.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            response
        } else {
            Response::new(Body::from(self.plain()?.into_owned()))
        };
        response.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        Ok(response)
    }
}

/// Whether the request takes gzip (`Accept-Encoding`): `gzip`, `x-gzip` or `*` with a weight above zero, unless gzip
/// itself weighs zero. No header — curl, a script — takes the file itself.
fn takes_gzip(request: &HeaderMap) -> bool {
    let (mut gzip, mut any) = (None, None);
    for value in request.get_all(header::ACCEPT_ENCODING) {
        for coding in value.to_str().unwrap_or_default().split(',') {
            let mut parts = coding.split(';');
            let name = parts.next().unwrap_or_default().trim();
            let weight = parts
                .filter_map(|part| part.split_once('='))
                .find(|(key, _)| key.trim().eq_ignore_ascii_case("q"))
                .map_or(1.0, |(_, value)| value.trim().parse::<f32>().unwrap_or(0.0));
            if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
                gzip = Some(weight > 0.0);
            } else if name == "*" {
                any = Some(weight > 0.0);
            }
        }
    }
    gzip.or(any).unwrap_or(false)
}

/// Where the panel's files come from: the program's own, or a test's.
pub trait PanelFiles: Sync {
    fn get(&self, path: &str) -> Option<PanelFile>;
}

/// The files built into the program.
pub struct BuiltIn;

#[cfg(not(debug_assertions))]
impl PanelFiles for BuiltIn {
    fn get(&self, path: &str) -> Option<PanelFile> {
        packed::get(path)
    }
}

/// A debug build serves panel/dist as it is now: a panel built again shows at the next reload, with no new build of
/// the gateway.
#[cfg(debug_assertions)]
impl PanelFiles for BuiltIn {
    fn get(&self, path: &str) -> Option<PanelFile> {
        OnDisk::get(path).map(|file| PanelFile { bytes: file.data, gzip: false })
    }
}

const NOT_BUILT: &str = "panel is not built: pnpm --filter panel build";

/// The types the panel's files are sent as; anything else is a download.
fn mime(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// The app shell, with its content policy (read from the page inflated).
pub fn index(files: &dyn PanelFiles, request: &HeaderMap) -> Result<Response, ApiError> {
    let html = files.get("index.html").ok_or_else(|| ApiError::not_found(NOT_BUILT))?;
    let policy = panel_policy(&String::from_utf8_lossy(&html.plain()?));
    let mut response = html.answer(request)?;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    if let Ok(policy) = HeaderValue::from_str(&policy) {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    Ok(response)
}

/// The page of an app's window (docs/plans/bus.md, «Окошки»): sandboxed by its own policy whatever frames it or
/// opens it, so the app's code never runs with the panel's origin — its key, its storage — and reaches no address by
/// itself: everything goes through the panel's bridge, as the bus allows.
pub fn frame(files: &dyn PanelFiles, request: &HeaderMap) -> Result<Response, ApiError> {
    let html = files.get("frame.html").ok_or_else(|| ApiError::not_found(NOT_BUILT))?;
    let policy = frame_policy(&String::from_utf8_lossy(&html.plain()?));
    let mut response = html.answer(request)?;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(policy) = HeaderValue::from_str(&policy) {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    Ok(response)
}

/// Built assets live under /panel/assets/*; anything else falls back to the app shell. They are public files: an
/// app's window, a frame of no origin, loads them across origins (`Access-Control-Allow-Origin: *`).
pub fn asset(files: &dyn PanelFiles, relative: &str, request: &HeaderMap) -> Result<Response, ApiError> {
    let relative = relative.trim_start_matches('/');
    if relative == "frame.html" {
        return frame(files, request);
    }
    let shell = relative.is_empty() || relative == "index.html" || relative.ends_with("/index.html");
    match files.get(relative).filter(|_| !shell) {
        Some(file) => {
            let mut response = file.answer(request)?;
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(relative)));
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
            Ok(response)
        }
        None => index(files, request),
    }
}

/// The inline scripts of a page by their hashes: the theme's boot, the import map (an import map is an inline script
/// to the content policy too). A script with a `src` is the panel's own file, under 'self'.
fn inline_hashes(html: &str) -> Vec<String> {
    let mut inline = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start + "<script".len()..];
        let Some(close) = after.find('>') else { break };
        let attributes = &after[..close];
        let body = &after[close + 1..];
        let Some(end) = body.find("</script>") else { break };
        let inline_kind = attributes.trim().is_empty() || attributes.trim() == "type=\"importmap\"";
        if inline_kind && !attributes.contains("src=") {
            inline.push(format!("'sha256-{}'", B64.encode(Sha256::digest(&body.as_bytes()[..end]))));
        }
        rest = &body[end + "</script>".len()..];
    }
    inline
}

/// The window's content policy: a sandbox of no origin (scripts only — no same origin, no forms, no popups, no
/// navigation of the panel), the panel's own scripts and the app's module, pictures from memory, and no connection
/// anywhere (`connect-src 'none'`): no fetch, no socket. Framed only by the panel.
pub fn frame_policy(html: &str) -> String {
    let scripts = format!("script-src 'self' {}", inline_hashes(html).join(" "));
    [
        "sandbox allow-scripts",
        "default-src 'none'",
        scripts.trim_end(),
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: blob:",
        "media-src 'self' data: blob:",
        "font-src 'self' data:",
        "connect-src 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'none'",
        "frame-ancestors 'self'",
    ]
    .join("; ")
}

/// The panel's content policy: its own scripts and the inline theme script by its hash, its own styles (inline style
/// attributes too), its own API; framed only by itself — the helper opens the station's sections in windows that are
/// the panel again, through the pass (docs/plans/desk.md, stage 4), and no other site may frame it. A script injected
/// into the page runs nowhere, and no form sends anything away from it (`form-action 'self'`). Audio may come from
/// blob: — the takes and samples of the own voices are fetched with the session's key and played from memory (an
/// <audio src> cannot send the key); so may images — the packages' avatars and the catalog's previews come the same
/// way; media never runs. The recording worklet is the panel's own file.
pub fn panel_policy(html: &str) -> String {
    // Inline scripts by their hash: the theme's boot and the import map (`<script type="importmap">`) that gives the
    // plugins' panel modules the panel's own Preact and SDK (docs/plans/plugins.md, stage 5).
    let scripts = format!("script-src 'self' {}", inline_hashes(html).join(" "));
    [
        "default-src 'self'",
        scripts.trim_end(),
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: blob:",
        "media-src 'self' blob:",
        "font-src 'self' data:",
        "connect-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'self'",
        "frame-ancestors 'self'",
    ]
    .join("; ")
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        // Someone who opens the gateway's bare address on first run finds the panel, not a 404 (the station's own
        // path is /uni.ws).
        .route("/", get(|| async { axum::response::Redirect::temporary("/panel") }))
        .merge(panel_routes(&BuiltIn))
        .merge(super::desk::routes())
        .merge(super::state::routes())
}

/// /panel over the panel's files: the shell, its assets, and the shell again for any other path.
fn panel_routes<S: Clone + Send + Sync + 'static>(files: &'static dyn PanelFiles) -> Router<S> {
    Router::new()
        .route("/panel", get(move |request: HeaderMap| async move { index(files, &request) }))
        .route("/panel/", get(move |request: HeaderMap| async move { index(files, &request) }))
        .route(
            "/panel/{*path}",
            get(move |Path(path): Path<String>, request: HeaderMap| async move { asset(files, &path, &request) }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use flate2::write::GzEncoder;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::io::Write as _;
    use tower::ServiceExt as _;

    /// A test's panel: each file kept as given, or gzip-compressed the way build.rs keeps text.
    struct Files(HashMap<&'static str, PanelFile>);

    impl PanelFiles for Files {
        fn get(&self, path: &str) -> Option<PanelFile> {
            self.0.get(path).cloned()
        }
    }

    /// Files kept as they are, the way a debug build reads panel/dist.
    fn plain(files: &[(&'static str, &'static str)]) -> Files {
        let kept = |text: &'static str| PanelFile { bytes: Cow::Borrowed(text.as_bytes()), gzip: false };
        Files(files.iter().map(|&(path, text)| (path, kept(text))).collect())
    }

    /// Files kept the way build.rs packs them: the text gzip-compressed, the `raw` ones (a font) as they are.
    fn packed(text: &[(&'static str, &'static str)], raw: &[(&'static str, &'static str)]) -> Files {
        let mut files = plain(raw);
        for &(path, text) in text {
            files.0.insert(path, PanelFile { bytes: Cow::Owned(gzip(text)), gzip: true });
        }
        files
    }

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    fn inflate(bytes: &[u8]) -> String {
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes).read_to_string(&mut text).unwrap();
        text
    }

    /// A request's headers: `Accept-Encoding` as given, or none.
    fn accepting(encodings: Option<&'static str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(encodings) = encodings {
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(encodings));
        }
        headers
    }

    /// What a browser says it takes.
    const BROWSER: Option<&str> = Some("gzip, deflate, br, zstd");
    const SHELL: &str = "<html><head><script>x()</script></head><body>пульт</body></html>";
    const SCRIPT: &str = "export const answer = 42; console.log('пульт', answer, answer, answer, answer);";
    const THEME: &str = "document.documentElement.dataset.theme = \"dark\";";

    /// An answer as it went: its status, its headers and its body.
    async fn sent(response: Response) -> (StatusCode, HeaderMap, Vec<u8>) {
        let (parts, body) = response.into_parts();
        (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
    }

    fn directive(policy: &str, name: &str) -> Option<String> {
        policy.split("; ").find(|part| part.starts_with(&format!("{name} "))).map(str::to_string)
    }

    #[test]
    fn runs_only_its_own_scripts_and_the_inline_theme_script_by_its_hash_never_a_blob_script() {
        let policy = panel_policy(&format!("<html><head><script>{THEME}</script></head></html>"));
        let hash = B64.encode(Sha256::digest(THEME.as_bytes()));
        assert_eq!(directive(&policy, "script-src"), Some(format!("script-src 'self' 'sha256-{hash}'")));
        assert!(!directive(&policy, "script-src").unwrap().contains("blob:"));
        assert_eq!(directive(&panel_policy("<html></html>"), "script-src").as_deref(), Some("script-src 'self'"));
        // The import map is hashed as an inline script; a module with a src is not.
        let map = r#"{"imports":{"preact":"/panel/sdk/preact.js"}}"#;
        let page = format!(
            r#"<script type="importmap">{map}</script><script type="module" crossorigin src="/panel/assets/a.js"></script>"#
        );
        let hash = B64.encode(Sha256::digest(map.as_bytes()));
        assert_eq!(directive(&panel_policy(&page), "script-src"), Some(format!("script-src 'self' 'sha256-{hash}'")));
    }

    #[test]
    fn plays_the_own_voices_takes_and_samples_from_blob_media_never_code() {
        let policy = panel_policy("<html></html>");
        assert_eq!(directive(&policy, "media-src").as_deref(), Some("media-src 'self' blob:"));
        assert_eq!(
            directive(&policy, "img-src").as_deref(),
            Some("img-src 'self' data: blob:"),
            "the packages' avatars"
        );
        assert_eq!(directive(&policy, "frame-ancestors").as_deref(), Some("frame-ancestors 'self'"));
    }

    /// An app's window: sandboxed by its own policy, no connection anywhere, the import map by its hash — read from
    /// the page inflated when it is kept compressed; never cached without asking.
    #[tokio::test]
    async fn the_window_page_is_a_sandbox_that_connects_nowhere() {
        let map = r#"{"imports":{"preact":"/panel/sdk/preact.js"}}"#;
        let page = format!(r#"<html><script type="importmap">{map}</script></html>"#);
        let policy = frame_policy(&page);
        assert!(policy.starts_with("sandbox allow-scripts; default-src 'none'"), "{policy}");
        let hash = B64.encode(Sha256::digest(map.as_bytes()));
        assert_eq!(directive(&policy, "script-src"), Some(format!("script-src 'self' 'sha256-{hash}'")));
        assert_eq!(directive(&policy, "connect-src").as_deref(), Some("connect-src 'none'"));
        assert_eq!(directive(&policy, "form-action").as_deref(), Some("form-action 'none'"));
        let files = plain(&[("frame.html", "<html>окно</html>"), ("assets/frame.js", "x")]);
        let served = asset(&files, "frame.html", &accepting(BROWSER)).unwrap();
        assert!(served.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .starts_with("sandbox allow-scripts"));
        let script = asset(&files, "assets/frame.js", &accepting(BROWSER)).unwrap();
        assert_eq!(script.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        let page: &'static str = Box::leak(page.into_boxed_str());
        let files = packed(&[("frame.html", page)], &[]);
        let (status, headers, body) = sent(asset(&files, "frame.html", &accepting(BROWSER)).unwrap()).await;
        assert_eq!((status, inflate(&body).as_str()), (StatusCode::OK, page));
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
        assert_eq!(headers[header::CONTENT_SECURITY_POLICY], policy.as_str());
    }

    /// Review S3: code that got into the page (a draft's panel.js) sends no form with the session elsewhere.
    #[test]
    fn sends_its_forms_only_to_itself() {
        let policy = panel_policy("<html></html>");
        assert_eq!(directive(&policy, "form-action").as_deref(), Some("form-action 'self'"));
        assert_eq!(directive(&policy, "base-uri").as_deref(), Some("base-uri 'none'"));
    }

    /// Files kept as they are (a debug build's panel/dist): each with its type, the app shell for anything else.
    #[tokio::test]
    async fn serves_its_assets_and_the_app_shell_for_anything_else() {
        let files = plain(&[
            ("index.html", "<html><script>x()</script></html>"),
            ("assets/app.js", "console.log(1)"),
            ("assets/font.woff2", "wOF2"),
        ]);
        let (status, headers, body) = sent(asset(&files, "assets/app.js", &accepting(BROWSER)).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(body, b"console.log(1)");
        assert!(headers.get(header::CONTENT_ENCODING).is_none() && headers.get(header::VARY).is_none());
        let font = asset(&files, "assets/font.woff2", &accepting(BROWSER)).unwrap();
        assert_eq!(font.headers()[header::CONTENT_TYPE], "font/woff2");
        let shell = index(&files, &accepting(BROWSER)).unwrap();
        assert!(shell.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("'sha256-"));
        for route in ["people", "settings/brain", "index.html", "", "assets/missing.js"] {
            let (status, headers, body) = sent(asset(&files, route, &accepting(BROWSER)).unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{route}");
            assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8", "{route}");
            assert!(body.starts_with(b"<html>"));
        }
    }

    /// A browser gets a script as build.rs keeps it — the gzip itself, not compressed again — and it inflates to
    /// exactly the file; the answer says it varies with `Accept-Encoding`.
    #[tokio::test]
    async fn answers_a_browser_with_the_gzip_as_kept_and_it_inflates_to_exactly_the_file() {
        let files = packed(&[("index.html", SHELL), ("assets/app.js", SCRIPT)], &[]);
        let (status, headers, body) = sent(asset(&files, "assets/app.js", &accepting(BROWSER)).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
        assert_eq!(headers[header::VARY], "Accept-Encoding");
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(body, gzip(SCRIPT));
        assert_eq!(inflate(&body), SCRIPT);
    }

    /// A client that takes no gzip — no `Accept-Encoding` at all (curl, a script), or gzip weighed zero — gets the
    /// file itself, inflated by the gateway.
    #[tokio::test]
    async fn answers_a_client_that_takes_no_gzip_with_the_file_itself() {
        let files = packed(&[("index.html", SHELL), ("assets/app.js", SCRIPT)], &[]);
        for encodings in [None, Some("identity"), Some("br, gzip;q=0"), Some("*;q=0")] {
            let (status, headers, body) = sent(asset(&files, "assets/app.js", &accepting(encodings)).unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{encodings:?}");
            assert!(headers.get(header::CONTENT_ENCODING).is_none(), "{encodings:?}");
            assert_eq!(headers[header::VARY], "Accept-Encoding");
            assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
            assert_eq!(String::from_utf8(body).unwrap(), SCRIPT);
        }
    }

    /// A font in woff2 is compressed already and kept as it is: everyone gets the same bytes, with no encoding and
    /// nothing that varies.
    #[tokio::test]
    async fn sends_a_file_kept_as_it_is_the_same_to_every_client() {
        let files = packed(&[("index.html", SHELL)], &[("assets/font.woff2", "wOF2 kept as it is")]);
        for encodings in [BROWSER, None] {
            let (status, headers, body) =
                sent(asset(&files, "assets/font.woff2", &accepting(encodings)).unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{encodings:?}");
            assert_eq!(headers[header::CONTENT_TYPE], "font/woff2", "{encodings:?}");
            assert!(headers.get(header::CONTENT_ENCODING).is_none() && headers.get(header::VARY).is_none());
            assert_eq!(body, b"wOF2 kept as it is");
        }
    }

    /// The single page's fallback: a screen's address, the shell by name and a file that is not there all get the
    /// app shell — gzip to a browser, inflated to anyone else — with its policy read from the page inflated.
    #[tokio::test]
    async fn answers_any_other_path_with_the_app_shell_kept_compressed_and_its_policy() {
        let files = packed(&[("index.html", SHELL), ("assets/app.js", SCRIPT)], &[]);
        let hash = format!("'sha256-{}'", B64.encode(Sha256::digest(b"x()")));
        for route in ["people", "settings/brain", "index.html", "", "assets/missing.js"] {
            let (status, headers, body) = sent(asset(&files, route, &accepting(BROWSER)).unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{route}");
            assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8", "{route}");
            assert_eq!(headers[header::CONTENT_ENCODING], "gzip", "{route}");
            assert_eq!(headers[header::VARY], "Accept-Encoding");
            assert!(headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains(&hash), "{route}");
            assert!(headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(), "the shell is the panel's own");
            assert_eq!(inflate(&body), SHELL);
            let (_, headers, body) = sent(asset(&files, route, &accepting(None)).unwrap()).await;
            assert!(headers.get(header::CONTENT_ENCODING).is_none(), "{route}");
            assert!(headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains(&hash), "{route}");
            assert_eq!(String::from_utf8(body).unwrap(), SHELL);
        }
    }

    /// HEAD through the panel's routes: the headers GET would send — the encoding, and the length of the body as it
    /// would go — and no body.
    #[tokio::test]
    async fn a_head_request_gets_the_answers_headers_and_no_body() {
        let files: &'static Files =
            Box::leak(Box::new(packed(&[("index.html", SHELL), ("assets/app.js", SCRIPT)], &[])));
        let routes = panel_routes::<()>(files);
        for (path, encodings, length, encoding) in [
            ("/panel/assets/app.js", BROWSER, gzip(SCRIPT).len(), Some("gzip")),
            ("/panel/assets/app.js", None, SCRIPT.len(), None),
            ("/panel/people", BROWSER, gzip(SHELL).len(), Some("gzip")),
            ("/panel", None, SHELL.len(), None),
        ] {
            let mut request = Request::head(path);
            if let Some(encodings) = encodings {
                request = request.header(header::ACCEPT_ENCODING, encodings);
            }
            let response = routes.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            let (status, headers, body) = sent(response).await;
            assert_eq!(status, StatusCode::OK, "{path} {encodings:?}");
            assert_eq!(headers[header::CONTENT_LENGTH], length.to_string(), "{path} {encodings:?}");
            assert_eq!(headers.get(header::CONTENT_ENCODING).map(|value| value.to_str().unwrap()), encoding);
            assert!(body.is_empty(), "{path} {encodings:?}");
        }
    }

    #[tokio::test]
    async fn says_the_panel_is_not_built_instead_of_failing() {
        let error = index(&plain(&[]), &accepting(BROWSER)).unwrap_err();
        assert_eq!((error.status, error.message.as_str()), (StatusCode::NOT_FOUND, NOT_BUILT));
        let error = asset(&plain(&[]), "assets/app.js", &accepting(None)).unwrap_err();
        assert_eq!((error.status, error.message.as_str()), (StatusCode::NOT_FOUND, NOT_BUILT));
        let nothing: &'static Files = Box::leak(Box::new(plain(&[])));
        let request = Request::head("/panel/people").body(Body::empty()).unwrap();
        assert_eq!(panel_routes::<()>(nothing).oneshot(request).await.unwrap().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn reads_whether_the_client_takes_gzip_from_its_accept_encoding() {
        for (encodings, takes) in [
            (None, false),
            (Some("gzip"), true),
            (Some("GZip"), true),
            (Some("x-gzip"), true),
            (Some("gzip, deflate, br, zstd"), true),
            (Some("br;q=1.0, gzip;q=0.8, *;q=0.1"), true),
            (Some("*"), true),
            (Some("deflate, *;q=0.5"), true),
            (Some("identity"), false),
            (Some("br, zstd"), false),
            (Some("gzip;q=0"), false),
            (Some("gzip; Q=0.000, *"), false),
            (Some("*;q=0"), false),
            (Some("gzip;q=nonsense"), false),
        ] {
            assert_eq!(takes_gzip(&accepting(encodings)), takes, "{encodings:?}");
        }
        let mut two = HeaderMap::new();
        two.append(header::ACCEPT_ENCODING, HeaderValue::from_static("br"));
        two.append(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(takes_gzip(&two), "Accept-Encoding said over two lines");
    }

    /// What build.rs packed into the program is panel/dist itself: every file there and nothing else, each reading
    /// back as exactly the file — scripts, styles and pages kept compressed, woff2 as it is. A panel not built packs to
    /// nothing. (No .woff gets there: the panel's own build stops on one, panel/vite.config.ts.)
    #[test]
    fn the_program_keeps_exactly_the_built_panel_with_its_text_compressed() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../panel/dist");
        let mut on_disk = Vec::new();
        let mut folders = vec![dist.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else {
                    on_disk.push(path.strip_prefix(&dist).unwrap().to_str().unwrap().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        assert_eq!(packed::FILES.iter().map(|file| file.path).collect::<Vec<_>>(), on_disk);
        for file in packed::FILES {
            let kept = packed::get(file.path).unwrap();
            let plain = std::fs::read(dist.join(file.path)).unwrap();
            assert_eq!(kept.plain().unwrap(), plain, "{}", file.path);
            let text = [".js", ".css", ".html", ".svg"].iter().any(|kind| file.path.ends_with(kind));
            if text && plain.len() > 1024 {
                assert!(kept.gzip && kept.bytes.len() < plain.len(), "{} is not kept compressed", file.path);
            }
            assert!(!(file.path.ends_with(".woff2") && kept.gzip), "{} is compressed already", file.path);
        }
    }
}
