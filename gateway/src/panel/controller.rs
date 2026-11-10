//! The control panel built in panel/ (Vite + Preact, `pnpm --filter panel build`), served at /panel from the files
//! built into the program (rust-embed over panel/dist; in a debug build they are read from that folder). Not built —
//! build.rs leaves the folder empty — the route says so instead of failing the gateway.

use std::borrow::Cow;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::platform::http::ApiError;

#[derive(rust_embed::RustEmbed)]
#[folder = "../panel/dist"]
struct Embedded;

/// Where the panel's files come from: the program's own, or a test's.
pub trait PanelFiles {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>>;
}

/// The files built into the program.
pub struct BuiltIn;

impl PanelFiles for BuiltIn {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        Embedded::get(path).map(|file| file.data)
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

/// The app shell, with its content policy.
pub fn index(files: &dyn PanelFiles) -> Result<Response, ApiError> {
    let html = files.get("index.html").ok_or_else(|| ApiError::not_found(NOT_BUILT))?;
    let policy = panel_policy(&String::from_utf8_lossy(&html));
    let mut response = Response::new(Body::from(html.into_owned()));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    if let Ok(policy) = HeaderValue::from_str(&policy) {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    Ok(response)
}

/// Built assets live under /panel/assets/*; anything else falls back to the app shell.
pub fn asset(files: &dyn PanelFiles, relative: &str) -> Result<Response, ApiError> {
    let relative = relative.trim_start_matches('/');
    let shell = relative.is_empty() || relative == "index.html" || relative.ends_with("/index.html");
    match files.get(relative).filter(|_| !shell) {
        Some(data) => Ok(([(header::CONTENT_TYPE, mime(relative))], data.into_owned()).into_response()),
        None => index(files),
    }
}

/// The panel's content policy: its own scripts and the inline theme script by its hash, its own styles (inline style
/// attributes too), its own API; framed only by itself — the helper opens the station's sections in windows that are
/// the panel again, through the pass (docs/plans/desk.md, stage 4), and no other site may frame it. A script injected
/// into the page runs nowhere, and no form sends anything away from it (`form-action 'self'`). Audio may come from
/// blob: — the takes and samples of the own voices are fetched with the session's key and played from memory (an
/// <audio src> cannot send the key); so may images — the packages' avatars and the catalog's previews come the same
/// way; media never runs. The recording worklet is the panel's own file.
pub fn panel_policy(html: &str) -> String {
    let mut inline = Vec::new();
    let mut rest = html;
    // Inline scripts by their hash: the theme's boot and the import map (`<script type="importmap">`) that gives the
    // plugins' panel modules the panel's own Preact and SDK (docs/plans/plugins.md, stage 5) — an import map is an
    // inline script to the content policy too. A script with a `src` is the panel's own file, under 'self'.
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
    let scripts = format!("script-src 'self' {}", inline.join(" "));
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
        .route("/panel", get(|| async { index(&BuiltIn) }))
        .route("/panel/", get(|| async { index(&BuiltIn) }))
        .route("/panel/{*path}", get(|Path(path): Path<String>| async move { asset(&BuiltIn, &path) }))
        .merge(super::desk::routes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use std::collections::HashMap;

    struct Files(HashMap<&'static str, &'static str>);

    impl PanelFiles for Files {
        fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
            self.0.get(path).map(|text| Cow::Borrowed(text.as_bytes()))
        }
    }

    const THEME: &str = "document.documentElement.dataset.theme = \"dark\";";

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

    /// Review S3: code that got into the page (a draft's panel.js) sends no form with the session elsewhere.
    #[test]
    fn sends_its_forms_only_to_itself() {
        let policy = panel_policy("<html></html>");
        assert_eq!(directive(&policy, "form-action").as_deref(), Some("form-action 'self'"));
        assert_eq!(directive(&policy, "base-uri").as_deref(), Some("base-uri 'none'"));
    }

    async fn text(response: Response) -> (StatusCode, String, String) {
        let status = response.status();
        let kind = response.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, kind.unwrap_or_default(), String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn serves_its_assets_and_the_app_shell_for_anything_else() {
        let files = Files(HashMap::from([
            ("index.html", "<html><script>x()</script></html>"),
            ("assets/app.js", "console.log(1)"),
            ("assets/font.woff2", "wOF2"),
        ]));
        let (status, kind, body) = text(asset(&files, "assets/app.js").unwrap()).await;
        assert_eq!(
            (status, kind.as_str(), body.as_str()),
            (StatusCode::OK, "text/javascript; charset=utf-8", "console.log(1)")
        );
        assert_eq!(text(asset(&files, "assets/font.woff2").unwrap()).await.1, "font/woff2");
        let shell = index(&files).unwrap();
        assert!(shell.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("'sha256-"));
        for route in ["people", "settings/brain", "index.html", "", "assets/missing.js"] {
            let (status, kind, body) = text(asset(&files, route).unwrap()).await;
            assert_eq!((status, kind.as_str()), (StatusCode::OK, "text/html; charset=utf-8"), "{route}");
            assert!(body.starts_with("<html>"));
        }
    }

    #[test]
    fn says_the_panel_is_not_built_instead_of_failing() {
        let error = index(&Files(HashMap::new())).unwrap_err();
        assert_eq!((error.status, error.message.as_str()), (StatusCode::NOT_FOUND, NOT_BUILT));
    }
}
