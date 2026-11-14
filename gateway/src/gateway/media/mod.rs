//! Our content for the station's player: files under CONTENT_DIR at /media/<path>, a stream the gateway relays at
//! /media/stream/<token> (stream.rs: the fallback for a stream the station keeps silent on), an app's own sound at
//! /media/apps/<id>/<path> (its `/stream/<path>` on localhost, while it runs here), and a package's own files at
//! /media/packages/<id>/<path> (its assets/: a plugin's while it is on, a character's or a theme's while it is
//! installed — the panel shows their avatars and plays their samples), with Range (the player seeks and resumes).
//! Only files inside the folder, no listings and no dot files; of a package nothing a browser would run (SVG, HTML,
//! scripts): it is served on the panel's own origin. Unsigned — the station's player cannot sign — so it answers the home network only: the folder
//! holds the family's voice messages too. The answer is the file itself, without the API's envelope.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::app::App;
use crate::platform::http::{ApiError, Caller, Zone};

pub mod stream;

pub fn router() -> Router<Arc<App>> {
    Router::new().route("/media/{*path}", get(media))
}

/// What a package's file may not be: anything a browser runs or renders as a page on the panel's origin.
const RUNNABLE: &[&str] = &["svg", "svgz", "html", "htm", "xhtml", "xml", "js", "mjs"];

/// A package's file at `path` under its assets, when it may be served.
/// `size=<px>` of a query: the longer side of a photo asked for, within reason.
fn size_asked(query: Option<&str>) -> Option<u32> {
    let value = query?.split('&').find_map(|pair| pair.strip_prefix("size="))?;
    value.parse::<u32>().ok().map(|side| side.clamp(64, crate::bus::providers::files::preview::SIDE_MOST))
}

pub fn package_file(assets: &Path, path: &str) -> Option<PathBuf> {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    if RUNNABLE.contains(&extension.as_str()) {
        return None;
    }
    media_file(assets, path)
}

/// The file a /media path names, when it may be served: inside the content folder, no dot part, not the family's
/// voice messages (played inline, never fetched by the player).
pub fn media_file(root: &Path, relative: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = relative.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|part| part.starts_with('.')) || parts[0] == "messages" {
        return None;
    }
    let candidate = parts.iter().fold(root.to_path_buf(), |path, part| path.join(part));
    if candidate.components().any(|component| matches!(component, Component::ParentDir)) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let file = candidate.canonicalize().ok()?;
    (file.starts_with(&root) && file != root && file.is_file()).then_some(file)
}

async fn media(
    State(app): State<Arc<App>>,
    caller: Caller,
    UrlPath(relative): UrlPath<String>,
    request: Request,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    // A stream the gateway relays for the station (stream.rs): one the station kept silent on.
    if let Some(token) = relative.strip_prefix("stream/") {
        return app.streams.serve(token).await;
    }
    // One file of the home folder an app played: the link `play` made (src/bus/providers/files), for six hours.
    if let Some(token) = relative.strip_prefix("folder/") {
        let files = app.bus.files().ok_or_else(|| ApiError::not_found("Not Found"))?;
        let file = files.played(token).ok_or_else(|| ApiError::not_found("Not Found"))?;
        // A photo for a window of the computer's screen (src/screen): `?size=` — made smaller, turned by its EXIF,
        // HEIC as JPEG, from the previews' cache.
        if let Some(side) = size_asked(request.uri().query()) {
            let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            if crate::bus::providers::files::kind_of(&name, false) == "image" {
                let made = tokio::task::spawn_blocking(move || files.preview_of(&file, side))
                    .await
                    .map_err(|error| ApiError::internal(format!("media: {error}")))?
                    .map_err(|error| ApiError::internal(format!("media: {error}")))?;
                let mut response = made.into_response();
                let headers = response.headers_mut();
                headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
                headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                return Ok(response);
            }
        }
        let mut response = match ServeFile::new(file).oneshot(request).await {
            Ok(response) => response.into_response(),
            Err(error) => return Err(ApiError::internal(format!("media: {error}"))),
        };
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    }
    // An app's own sound, while it runs here: its /stream/<path> on localhost, nothing else of its server.
    if let Some(rest) = relative.strip_prefix("apps/") {
        let (id, path) = rest.split_once('/').ok_or_else(|| ApiError::not_found("Not Found"))?;
        let url = app.plugins.app_stream(id, path).await.ok_or_else(|| ApiError::not_found("Not Found"))?;
        return stream::app_sound(&url).await;
    }
    let file = match relative.strip_prefix("packages/") {
        // A package's own files (its assets/): of a plugin that is on, of a character or a theme installed; only
        // inside that folder.
        Some(rest) => rest.split_once('/').and_then(|(id, path)| package_file(&app.plugins.media_assets(id)?, path)),
        None => media_file(&app.config.skills.content_dir, &relative),
    };
    let file = file.ok_or_else(|| ApiError::not_found("Not Found"))?;
    let mut response = match ServeFile::new(file).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(error) => return Err(ApiError::internal(format!("media: {error}"))),
    };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_files_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("music/Альбом")).unwrap();
        std::fs::create_dir_all(root.join("messages")).unwrap();
        std::fs::write(root.join("music/Альбом/Песня.mp3"), b"mp3").unwrap();
        std::fs::write(root.join("messages/мама.ogg"), b"ogg").unwrap();
        std::fs::write(root.join(".secret"), b"x").unwrap();
        assert!(media_file(root, "music/Альбом/Песня.mp3").is_some());
        assert!(media_file(root, "messages/мама.ogg").is_none(), "voice messages are never fetched");
        assert!(media_file(root, ".secret").is_none());
        assert!(media_file(root, "music/../../etc/passwd").is_none());
        assert!(media_file(root, "music").is_none(), "no listings");
        assert!(media_file(root, "").is_none());
        assert!(media_file(root, "nothing.mp3").is_none());
        std::fs::write(root.join("music/avatar.webp"), b"webp").unwrap();
        std::fs::write(root.join("music/page.SVG"), b"<svg/>").unwrap();
        assert!(package_file(root, "music/avatar.webp").is_some());
        assert!(package_file(root, "music/page.SVG").is_none(), "nothing a browser runs on the panel's origin");
    }
}
