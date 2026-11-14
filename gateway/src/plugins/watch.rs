//! «Смотри и выбирай» (docs/plans/feed.md, stage 10): an app's video with its fork on a page of this gateway, by a
//! link anyone with it may open — `/watch/<token>`. The page plays the video with seeking and takes a vote ① or ②
//! (a form, no script); the votes are counted here and told to the app (its function `vote`, in the background).
//! A link lives 30 days; an app keeps up to 200. The page runs nothing (`script-src 'none'`) and shows only that video.
//!
//! The page is on this gateway: at home by its address, outside by the gateway's own address there. A page on our
//! hosting needs our server (the owner's step); the link format stays the same.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Form, Path as UrlPath, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;

const DAYS: i64 = 30;
const PER_APP: usize = 200;

/// A page: whose video, which file of its folder, the fork, the votes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchPage {
    pub app: String,
    pub file: String,
    pub title: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub a: Option<String>,
    #[serde(default)]
    pub b: Option<String>,
    #[serde(default)]
    pub votes: [u64; 2],
    pub created_ms: i64,
}

/// The pages, one JSON file each in DATA_DIR/watch.
pub struct WatchPages {
    dir: PathBuf,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn token_ok(token: &str) -> bool {
    token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit())
}

impl WatchPages {
    pub fn new(dir: PathBuf) -> WatchPages {
        WatchPages { dir }
    }

    fn path(&self, token: &str) -> PathBuf {
        self.dir.join(format!("{token}.json"))
    }

    pub fn get(&self, token: &str) -> Option<WatchPage> {
        if !token_ok(token) {
            return None;
        }
        let page: WatchPage = serde_json::from_slice(&std::fs::read(self.path(token)).ok()?).ok()?;
        (now_ms() - page.created_ms < DAYS * 86_400_000).then_some(page)
    }

    fn put(&self, token: &str, page: &WatchPage) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let tmp = self.dir.join(format!(".{token}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(page).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, self.path(token)).map_err(|error| error.to_string())
    }

    /// A new page for an app's video: its token. Pages past their days and an app's oldest past 200 leave.
    pub fn create(&self, page: WatchPage) -> Result<String, String> {
        let mine: Vec<(String, i64)> = self
            .list()
            .into_iter()
            .filter(|(_, one)| one.app == page.app)
            .map(|(token, one)| (token, one.created_ms))
            .collect();
        let mut sorted = mine;
        sorted.sort_by_key(|(_, at)| *at);
        while sorted.len() >= PER_APP {
            let (token, _) = sorted.remove(0);
            let _ = std::fs::remove_file(self.path(&token));
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.put(&token, &page)?;
        Ok(token)
    }

    fn list(&self) -> Vec<(String, WatchPage)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(token) = name.strip_suffix(".json") else { continue };
            match self.get(token) {
                Some(page) => out.push((token.to_string(), page)),
                None => {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        out
    }

    /// A vote ① (0) or ② (1): the counts after it.
    pub fn vote(&self, token: &str, side: usize) -> Option<WatchPage> {
        let mut page = self.get(token)?;
        page.votes[side.min(1)] += 1;
        self.put(token, &page).ok()?;
        Some(page)
    }

    /// The pages of an app gone with it.
    pub fn forget(&self, app: &str) {
        for (token, page) in self.list() {
            if page.app == app {
                let _ = std::fs::remove_file(self.path(&token));
            }
        }
    }
}

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/watch/{token}", get(page))
        .route("/watch/{token}/video", get(video))
        .route("/watch/{token}/vote", post(vote))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const POLICY: &str = "default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

fn render(token: &str, page: &WatchPage, voted: Option<usize>) -> String {
    let total = page.votes[0] + page.votes[1];
    let share = |side: usize| (page.votes[side] * 100).checked_div(total).unwrap_or(0);
    let choice = match (&page.q, &page.a, &page.b) {
        (Some(q), Some(a), Some(b)) => {
            let counts = voted.map(|mine| {
                format!(
                    "<p class=\"counts\">Ваш выбор — {}. Пока так: ① {}% · ② {}% ({total})</p>",
                    if mine == 0 { "①" } else { "②" },
                    share(0),
                    share(1)
                )
            });
            format!(
                "<h2>{}</h2><form method=\"post\" action=\"/watch/{token}/vote\"><button name=\"choice\" value=\"a\">① {}</button><button name=\"choice\" value=\"b\">② {}</button></form>{}",
                escape(q),
                escape(a),
                escape(b),
                counts.unwrap_or_default()
            )
        }
        _ => String::new(),
    };
    format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title>\
         <style>body{{margin:0;background:#0b0b0d;color:#f4f4f0;font:16px/1.4 system-ui,sans-serif;display:flex;justify-content:center}}\
         main{{width:min(100%,440px);padding:16px;box-sizing:border-box}}video{{width:100%;aspect-ratio:9/16;background:#000;border-radius:14px}}\
         h1{{font-size:20px;margin:12px 0 4px}}h2{{font-size:18px;margin:16px 0 8px}}form{{display:flex;flex-direction:column;gap:8px}}\
         button{{font:inherit;padding:12px 14px;border-radius:12px;border:0;background:#c8ff3d;color:#0b0b0d;font-weight:700;text-align:left}}\
         .counts{{opacity:.8}}footer{{margin-top:20px;opacity:.6;font-size:13px}}</style></head>\
         <body><main><video src=\"/watch/{token}/video\" controls playsinline preload=\"metadata\"></video><h1>{title}</h1>{choice}\
         <footer>Снято на компьютере в DuoWave · duowave.ai</footer></main></body></html>",
        title = escape(&page.title),
    )
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(POLICY));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

/// Which side this browser voted for on this page: its cookie.
fn voted_here(headers: &HeaderMap, token: &str) -> Option<usize> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let key = format!("watch-{}=", &token[..12]);
    cookies.split(';').map(str::trim).find_map(|cookie| cookie.strip_prefix(&key)).and_then(|side| match side {
        "a" => Some(0),
        "b" => Some(1),
        _ => None,
    })
}

async fn page(State(app): State<Arc<App>>, UrlPath(token): UrlPath<String>, headers: HeaderMap) -> Response {
    match app.watch.get(&token) {
        Some(page) => secured(Html(render(&token, &page, voted_here(&headers, &token))).into_response()),
        None => (StatusCode::NOT_FOUND, "Ссылка устарела или её нет.").into_response(),
    }
}

async fn video(State(app): State<Arc<App>>, UrlPath(token): UrlPath<String>, request: Request) -> Response {
    let Some(page) = app.watch.get(&token) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(file) = file_of(&app.files, &page.app, &page.file) else { return StatusCode::NOT_FOUND.into_response() };
    // The one media engine (gateway/media): Range for seeking, the type by the bytes, nothing a browser runs.
    match crate::gateway::media::engine::serve(file, crate::gateway::media::Allow::Media, request, "no-store").await {
        Ok(response) => secured(response),
        Err(error) => error.into_response(),
    }
}

fn file_of(files: &crate::plugins::files::PluginFiles, app: &str, path: &str) -> Option<PathBuf> {
    let file = files.inside(app, path).ok()?;
    (file.is_file() && Path::new(path).extension().is_some_and(|ext| ext == "mp4")).then_some(file)
}

#[derive(Deserialize)]
struct Vote {
    choice: String,
}

async fn vote(
    State(app): State<Arc<App>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
    Form(form): Form<Vote>,
) -> Response {
    let side = match form.choice.as_str() {
        "a" => 0,
        "b" => 1,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    // One vote per browser and page: a second only changes nothing.
    if voted_here(&headers, &token).is_none() {
        let Some(page) = app.watch.vote(&token, side) else { return StatusCode::NOT_FOUND.into_response() };
        let plugins = app.plugins.clone();
        tokio::spawn(async move {
            let input = json!({ "id": page.id, "choice": if side == 0 { "a" } else { "b" }, "votes": page.votes });
            let _: Result<Value, _> =
                plugins.call(&page.app, "vote", input, crate::plugins::runtime::CallKind::Background).await;
        });
    }
    let mut response = Redirect::to(&format!("/watch/{token}")).into_response();
    let cookie = format!(
        "watch-{}={}; Path=/watch/{token}; Max-Age={}; SameSite=Lax; HttpOnly",
        &token[..12.min(token.len())],
        if side == 0 { "a" } else { "b" },
        DAYS * 86_400
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_page_counts_votes_and_lets_an_app_keep_two_hundred() {
        let dir = tempfile::tempdir().unwrap();
        let pages = WatchPages::new(dir.path().to_path_buf());
        let page = |n: i64| WatchPage {
            app: "feed".into(),
            file: "share/a.mp4".into(),
            title: format!("Серия {n}"),
            id: Some(format!("a-{n}")),
            q: Some("ОТКРЫВАЕШЬ?".into()),
            a: Some("ОТКРЫТЬ".into()),
            b: Some("НЕТ".into()),
            votes: [0, 0],
            created_ms: now_ms() + n,
        };
        let token = pages.create(page(0)).unwrap();
        assert!(token_ok(&token));
        assert_eq!(pages.vote(&token, 1).unwrap().votes, [0, 1]);
        assert_eq!(pages.vote(&token, 0).unwrap().votes, [1, 1]);
        assert!(pages.get("../../etc/passwd").is_none());
        assert!(render(&token, &pages.get(&token).unwrap(), Some(0)).contains("① 50% · ② 50% (2)"));
        assert!(render(&token, &page(1), None).contains("ОТКРЫВАЕШЬ?"));
        for n in 1..=PER_APP as i64 {
            pages.create(page(n)).unwrap();
        }
        assert!(pages.get(&token).is_none(), "the oldest left past two hundred");
        pages.forget("feed");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
