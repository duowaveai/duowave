//! `/media`: photos, videos and sounds to whoever shows or plays them — all through one engine (engine.rs, «Один
//! движок показа» in docs/plans/feed.md):
//!
//! - `/media/v/<key>` — a file the engine gave a key to (the panel's viewer and «Папки», an app's window and cards,
//!   the computer's screen, the station's player): the key is the pass, from wherever the panel works — unless it was
//!   given for home only. `?size=<px>` — its picture: a photo made small, a video's cover frame.
//!   `/media/folder/<key>` is the same key by its older address.
//! - `/media/<path>` — the household's music and tales under CONTENT_DIR, for the station's player; unsigned (the
//!   player cannot sign), so the home network only. Not the family's voice messages: they are played inline.
//! - `/media/packages/<id>/<path>` — a package's own files (its `assets/`: a plugin's while it is on, a character's or
//!   a theme's while it is installed — avatars, samples, sounds); anything but what a browser runs.
//! - `/media/stream/<token>` — a stream the gateway relays for the station (stream.rs), and `/media/apps/<id>/<path>`
//!   an app's own sound from its `/stream/<path>` on localhost while it runs here: these are no files.
//!
//! Only files inside their folder, no listings, no dot files; the answer is the file itself, without the API's
//! envelope, with Range (the player seeks and resumes).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, Request, State};
use axum::routing::{get, post};
use axum::Router;

use crate::app::App;
use crate::platform::http::{ApiError, Caller, Zone};

pub mod engine;
pub mod kind;
pub mod probe;
pub mod routes;
pub mod stream;

pub use engine::{Allow, Grant, Link, MediaEngine};

pub fn router() -> Router<Arc<App>> {
    Router::new().route("/media/{*path}", get(media)).route("/api/v1/media/links", post(routes::links))
}

/// `size=<px>` of a query: the longer side of a picture asked for, within reason.
fn size_asked(query: Option<&str>) -> Option<u32> {
    let value = query?.split('&').find_map(|pair| pair.strip_prefix("size="))?;
    value.parse::<u32>().ok().map(|side| side.clamp(64, crate::bus::providers::files::preview::SIDE_MOST))
}

/// A package's file at `path` under its assets, when it may be served (the engine refuses what a browser runs).
pub fn package_file(assets: &Path, path: &str) -> Option<PathBuf> {
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
) -> Result<axum::response::Response, ApiError> {
    // A key the engine gave: the key is the pass.
    if let Some(key) = relative.strip_prefix("v/").or_else(|| relative.strip_prefix("folder/")) {
        return keyed(&app, &caller, key, request).await;
    }
    caller.zone_at_least(Zone::Home)?;
    // A stream the gateway relays for the station (stream.rs): one the station kept silent on.
    if let Some(token) = relative.strip_prefix("stream/") {
        return app.streams.serve(token).await;
    }
    // An app's own sound, while it runs here: its /stream/<path> on localhost, nothing else of its server.
    if let Some(rest) = relative.strip_prefix("apps/") {
        let (id, path) = rest.split_once('/').ok_or_else(|| ApiError::not_found("Not Found"))?;
        let url = app.plugins.app_stream(id, path).await.ok_or_else(|| ApiError::not_found("Not Found"))?;
        return stream::app_sound(&url, request.headers().get(axum::http::header::RANGE).cloned()).await;
    }
    // A package's own files (its assets/): of a plugin that is on, of a character or a theme installed.
    if let Some(rest) = relative.strip_prefix("packages/") {
        let file = rest.split_once('/').and_then(|(id, path)| package_file(&app.plugins.media_assets(id)?, path));
        let file = file.ok_or_else(|| ApiError::not_found("Not Found"))?;
        return engine::serve(file, Allow::Assets, request, "no-store").await;
    }
    let file = media_file(&app.config.skills.content_dir, &relative).ok_or_else(|| ApiError::not_found("Not Found"))?;
    engine::serve(file, Allow::Media, request, "no-store").await
}

/// A file by the engine's key: on the home network only when given so. An app's keys close when the app is removed
/// (plugins/routes.rs) — a workshop's draft has its keys too, so no «installed» check here.
async fn keyed(
    app: &Arc<App>,
    caller: &Caller,
    key: &str,
    request: Request,
) -> Result<axum::response::Response, ApiError> {
    // A Турбо's file (docs/plans/turbo-devices.md, «Папки Турбо»): its engine's key named with its place — the fetch
    // goes to the Турбо's own address, the station passing nothing through.
    if let Some(rest) = key.strip_prefix(crate::home::turbo_devices::TURBO_KEY) {
        let (place, key) = rest.split_once('.').ok_or_else(|| ApiError::not_found("Not Found"))?;
        let mut url = app.house.media_url(place, key).ok_or_else(|| ApiError::not_found("Not Found"))?;
        if let Some(query) = request.uri().query() {
            url = format!("{url}?{query}");
        }
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::TEMPORARY_REDIRECT)
            .header(axum::http::header::LOCATION, url)
            .header(axum::http::header::CACHE_CONTROL, "no-store")
            .body(axum::body::Body::empty())
            .map_err(ApiError::internal);
    }
    let files = app.bus.files().ok_or_else(|| ApiError::not_found("Not Found"))?;
    let ticket = files.media().ticket(key).ok_or_else(|| ApiError::not_found("Not Found"))?;
    if ticket.home_only && !matches!(caller.zone, Zone::Local | Zone::Home) {
        return Err(ApiError::not_found("Not Found"));
    }
    // Its picture: a photo made small, a video's cover frame.
    if let Some(side) = size_asked(request.uri().query()) {
        if ticket.sniffed.kind != kind::Kind::Audio {
            let made = files.media().picture(ticket.path.clone(), ticket.sniffed, side).await;
            return made.map(|bytes| engine::jpeg(bytes, "private, max-age=3600")).map_err(ApiError::not_found);
        }
    }
    engine::serve(ticket.path, Allow::Media, request, "private, max-age=3600").await
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
    }

    #[test]
    fn asks_a_picture_within_reason() {
        assert_eq!(size_asked(Some("size=256")), Some(256));
        assert_eq!(size_asked(Some("a=1&size=99999")), Some(crate::bus::providers::files::preview::SIDE_MOST));
        assert_eq!(size_asked(Some("size=1")), Some(64));
        assert_eq!(size_asked(Some("size=big")), None);
        assert_eq!(size_asked(None), None);
    }
}
