//! The helper's pass to the station (docs/plans/desk.md, stage 4 — «идея 3» of docs/plans/placement-per-station.md):
//! the panel on the computer opens the station's own sections through `/station/<key>/…`, from the home network only.
//! The computer's gateway checks the person, then forwards the request to the station's gateway with the pair's key
//! and the person's role; the station takes it as «Помощник» with that role's rights (platform/http/caller.rs) — a
//! member at the computer sees there what a member sees. Its answers come back as they are, the live feed streamed.
//!
//! A GET's answer is kept on disk (`station-copies/<key>/`, each role its own, the newest few hundred, gone with the
//! station's record): while the station is away, the panel sees the last copy, marked by `x-duowave-copy-at`, and a
//! change is refused in words — it can be made when the station returns. While the station's settings live on this
//! computer, a change of them is refused too: the next sending from here would overwrite it. A plugin's call is a
//! reading here (its widgets read their data with it), and the station's plugins' panel parts and the workshop's
//! previews pass without the session: `import()` and `<link>` carry no key, and the station serves them to anyone.

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::core::identity::permissions::STATION_VIEW;
use crate::core::identity::principal::{Principal, PrincipalKind};
use crate::core::identity::roles::Role;
use crate::platform::http::{ApiError, Caller, Zone};
use crate::settings::definitions::SettingsHome;
use crate::utils::time::now_ms;

/// The header a copy carries: when the station answered it, unix ms.
pub const COPY_AT: &str = "x-duowave-copy-at";
/// Who on the computer made the change (for the station's history), and their role (for their rights there).
use super::peer::{HELPER_PERSON, HELPER_ROLE};
/// A body to pass on: the panel's requests are small; a voice take or a package is the largest.
const BODY_LIMIT: usize = 32 * 1024 * 1024;
/// The station answers in the home network fast, or it is away.
const CONNECT: Duration = Duration::from_secs(3);
/// A reading waits for the station this long: the panel's widgets read it on a timer, and a station that hangs must
/// not hold the browser's few connections to this computer (docs/plans/desk-after-station.md, А).
const READ_WAIT: Duration = Duration::from_secs(8);
/// A change — a package installed, a voice take sent — may take long.
const CHANGE_WAIT: Duration = Duration::from_secs(120);
/// The copies of one station kept at most: past them the oldest go.
const COPIES_KEPT: usize = 400;
/// An answer this big or bigger is not copied: a page of the journal is tens of KB.
const COPY_LIMIT: usize = 2 * 1024 * 1024;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/station/{key}/{*rest}", any(pass)).layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
}

/// The client of the pass, and of the relay of the station's live channel (relay.rs): the home network's connect time.
pub(crate) fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            crate::utils::http::client_builder().connect_timeout(CONNECT).build().expect("the HTTP client builds")
        })
        .clone()
}

/// What may pass: the panel's API, its health, the station's plugins' panel parts, the workshop's previews and its
/// media — nothing else.
fn passable(rest: &str) -> bool {
    rest == "health"
        || ["api/v1/", "panel-plugins/", "panel-preview/", "media/"].iter().any(|prefix| rest.starts_with(prefix))
}

/// What the station serves without a key: its plugins' panel parts and the workshop's previews by their token. The
/// panel's `import()` and `<link>` send no key, so these pass without the session check — from home only, on the same
/// whitelist.
fn public_file(rest: &str) -> bool {
    ["panel-plugins/", "panel-preview/"].iter().any(|prefix| rest.starts_with(prefix))
}

/// A plugin's call (`api/v1/packages/<id>/call/<function>`): the plugin's widgets read their data with it, so here it
/// is a reading — the station checks the call's own right with the person's role, and the computer's sending
/// overwrites nothing of it.
fn plugin_call(rest: &str) -> bool {
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["api", "v1", "packages", id, "call", function] => !id.is_empty() && !function.is_empty(),
        _ => false,
    }
}

/// A path that stays where it says, before the whitelist reads it: no empty segment, no «.» or «..» — nor their
/// percent-encoded forms or the same with spaces around, which the address of the request reads as the same — and no
/// backslash, query, fragment, control character or encoded slash hidden in a segment (the router has decoded the
/// path once: `..%2F` came as `../`). The address made of it is checked against the whitelist once more (`pass`).
fn plain_path(rest: &str) -> bool {
    !rest.contains(|c: char| matches!(c, '\\' | '?' | '#') || c.is_control())
        && rest.split('/').all(|segment| {
            let segment = segment.to_ascii_lowercase();
            let dotted = segment.trim().replace("%2e", ".");
            !matches!(dotted.as_str(), "" | "." | "..") && !segment.contains("%2f") && !segment.contains("%5c")
        })
}

/// The role the station gives the person at the computer: a person's own; a key's (no role) — a guest's.
fn helper_role(principal: &Principal) -> Role {
    match (principal.kind, principal.role) {
        (PrincipalKind::User, Some(role)) => role,
        _ => Role::Guest,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Copy {
    at: u64,
    status: u16,
    content_type: String,
    body: String,
}

/// A station's copies: `station-copies/<key>` in the state folder — never the folder itself or above it.
fn copies_of(state_dir: Option<&FsPath>, key: &str) -> Option<PathBuf> {
    let folder: String =
        key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    if folder.chars().all(|c| c == '.') {
        return None;
    }
    state_dir.map(|dir| dir.join("station-copies").join(folder))
}

fn copies_dir(app: &App, key: &str) -> Option<PathBuf> {
    copies_of(app.config.skills.state_dir.as_deref(), key)
}

/// The station's record is gone (src/placement/watch.rs): what it showed through the pass goes with it.
pub(crate) fn forget_copies(state_dir: Option<&FsPath>, key: &str) {
    if let Some(dir) = copies_of(state_dir, key).filter(|dir| dir.is_dir()) {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(target: "placement", "the station {key}'s copies stay in {}: {error}", dir.display());
        }
    }
}

/// A copy is the role's: what the station showed an admin is never shown to a member while it is away.
fn copy_path(app: &App, key: &str, role: Role, rest: &str, query: Option<&str>) -> Option<PathBuf> {
    copies_dir(app, key).map(|dir| dir.join(copy_name(role, rest, query)))
}

fn copy_name(role: Role, rest: &str, query: Option<&str>) -> String {
    let name = hex::encode(Sha256::digest(format!("{rest}?{}", query.unwrap_or("")).as_bytes()));
    format!("{}-{}.json", role.as_str(), &name[..32])
}

fn read_copy(path: &PathBuf) -> Option<Copy> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// A copy written; a new one beside the station's others makes the oldest go past `COPIES_KEPT`.
fn write_copy(path: &FsPath, copy: &Copy) {
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    let new = !path.exists();
    if let Ok(bytes) = serde_json::to_vec(copy) {
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, path).is_ok() && new {
            keep_newest(dir, COPIES_KEPT);
        }
    }
}

/// Of the copies in the folder, the newest `kept` stay (by the time each was written); the others go.
fn keep_newest(dir: &FsPath, kept: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut copies: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    if copies.len() <= kept {
        return;
    }
    copies.sort();
    let gone = copies.len() - kept;
    for (_, path) in copies.into_iter().take(gone) {
        let _ = std::fs::remove_file(path);
    }
}

fn away(name: &str) -> String {
    format!("Колонка «{name}» не на связи: изменения можно сделать, когда она вернётся.")
}

async fn pass(
    State(app): State<Arc<App>>,
    caller: Caller,
    method: Method,
    headers: HeaderMap,
    Path((key, rest)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    // From the home network only: the station sees this computer's address and would take the person for one at
    // home — its own rules «только из дома» (its plugins' code, its connectors, the pair) would not hold.
    caller.zone_at_least(Zone::Home)?;
    let reading = matches!(method, Method::GET | Method::HEAD);
    // The station's plugins' panel parts and the workshop's previews are public there and come without a key
    // (`import()`, `<link>`): no session asked. Everything else — the person on the computer: reading needs the right
    // to see the station; a change — an admin here. On the station they have their role's rights (HELPER_ROLE), no
    // more. A plugin's call is a reading: the station checks its right with that role.
    let principal = if reading && public_file(&rest) { None } else { Some(caller.permitted(STATION_VIEW)?.clone()) };
    let changing = !reading && !(method == Method::POST && plugin_call(&rest));
    if changing && principal.as_ref().and_then(|principal| principal.role) != Some(Role::Admin) {
        return Err(ApiError::forbidden("Править настройки колонки отсюда может администратор."));
    }
    if !plain_path(&rest) || !passable(&rest) {
        return Err(ApiError::not_found("Not Found"));
    }
    // Only a station with a placement here: an unknown key makes no watch.
    let Some(gateway) = app.placement.station().and_then(|stations| stations.find(&key)) else {
        return Err(ApiError::not_found("Такой колонки у этого компьютера нет."));
    };
    let record = gateway.record();
    // Its settings live here and go to it from here: a change of them made in its window would be overwritten by the
    // next sending — they go back to the station first. That is every change `routes::sent_over` names, the workshop's
    // model among them. What does not travel goes on: a plugin's call, the player, the workshop's drafts, the mode (the
    // station's own: only a move brings the computer's), «Спросить мозг» and the checks that write nothing.
    if changing
        && super::routes::sent_over(&format!("/{rest}"))
        && record.as_ref().is_some_and(|record| record.settings_home == SettingsHome::Pc)
    {
        return Err(ApiError::conflict(super::routes::SETTINGS_ON_PC));
    }
    let Some(peer) = gateway.client() else {
        return Err(ApiError::conflict("Колонка не связана с этим компьютером: нет ключа пары."));
    };
    let name = record.and_then(|record| record.device).unwrap_or_else(|| key.clone());
    let path = format!("/{rest}{}", query.as_deref().map(|query| format!("?{query}")).unwrap_or_default());
    // The address as it will be asked, checked once more: nothing in it may lead out of the whitelist.
    let url = reqwest::Url::parse(&format!("{}{path}", peer.base()))
        .ok()
        .filter(|url| passable(url.path().trim_start_matches('/')))
        .ok_or_else(|| ApiError::not_found("Not Found"))?;
    let role = principal.as_ref().map(helper_role);
    let copy =
        role.filter(|_| method == Method::GET).and_then(|role| copy_path(&app, &key, role, &rest, query.as_deref()));

    let mut request = client().request(method.clone(), url);
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::RANGE] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    // A public file goes as anyone's at home: no key, nobody named.
    if let (Some(principal), Some(role)) = (&principal, role) {
        request = request
            .bearer_auth(peer.key())
            .header(HELPER_PERSON, principal.name.chars().filter(|c| !c.is_control()).collect::<String>())
            .header(HELPER_ROLE, role.as_str());
    }
    if changing || !body.is_empty() {
        request = request.body(body);
    }
    // The live feed through the pass is an older panel's: the new one hears the station on its own socket (relay.rs).
    let live_feed = rest == "api/v1/live";
    if !live_feed {
        request = request.timeout(if changing { CHANGE_WAIT } else { READ_WAIT });
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(target: "placement", "pass to the station {key}: {error}");
            // Away: the last copy of what was asked, read-only; a change waits for the station.
            if let Some(found) = copy.as_ref().and_then(read_copy) {
                return Ok(copy_response(found));
            }
            // It took the request and did not answer in time: it is there, but slow — not «away».
            if error.is_timeout() && !error.is_connect() {
                return Err(ApiError::unavailable(format!(
                    "Колонка «{name}» не ответила вовремя — попробуйте ещё раз."
                )));
            }
            return Err(ApiError::unavailable(away(&name)));
        }
    };

    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    // The live feed and anything else that streams goes through as it comes.
    if live_feed || content_type.starts_with("text/event-stream") || content_type.starts_with("audio/") {
        let stream = response.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other));
        let mut out = Response::new(Body::from_stream(stream));
        *out.status_mut() = status;
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            out.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        out.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return Ok(out);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("Шлюз колонки оборвал ответ: {error}")))?;
    let worth = status.is_success() && content_type.starts_with("application/json") && bytes.len() < COPY_LIMIT;
    if let (Some(path), true) = (copy.as_ref(), worth) {
        write_copy(
            path,
            &Copy {
                at: now_ms(),
                status: status.as_u16(),
                content_type: content_type.clone(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            },
        );
    }
    let mut out = Response::new(Body::from(bytes));
    *out.status_mut() = status;
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        out.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    out.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // An app's window is a sandboxed frame of no origin (docs/plans/bus.md, «Окошки»): it loads a station's panel part
    // across origins, as the station itself allows it. The pass rebuilds the answer, so the allowance goes on here.
    if public_file(&rest) {
        out.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    }
    Ok(out)
}

fn copy_response(copy: Copy) -> Response {
    let mut out = (StatusCode::from_u16(copy.status).unwrap_or(StatusCode::OK), copy.body).into_response();
    if let Ok(value) = HeaderValue::from_str(&copy.content_type) {
        out.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&copy.at.to_string()) {
        out.headers_mut().insert(COPY_AT, value);
    }
    out.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    out
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Mutex;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::{json, Map, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::access::auth_service::LoginResult;
    use crate::config::{Config, Env};
    use crate::core::availability::Placement;
    use crate::core::identity::principal::user_principal;
    use crate::settings::definitions::StationPlacement;

    #[test]
    fn passes_the_panels_api_its_health_plugins_previews_and_media_only() {
        assert!(passable("api/v1/brain"));
        assert!(passable("health"));
        assert!(passable("panel-plugins/radio/1.2.0/panel.js"));
        assert!(passable("panel-preview/t0k3n/panel.js"));
        assert!(passable("media/radio/cover.png"));
        assert!(!passable("uni.ws"));
        assert!(!passable("debug/sessions"));
        assert!(!passable("home/v1/llm/chat/completions"));
        assert!(!passable("panel/index.html"));
        // Without the session: the station's public files only.
        assert!(public_file("panel-plugins/radio/1.2.0/panel.css") && public_file("panel-preview/t0k3n/panel.js"));
        assert!(!public_file("api/v1/packages/panel") && !public_file("media/radio/cover.png"));
        // A plugin's call is a reading; its settings, its other routes are not.
        assert!(plugin_call("api/v1/packages/shopping/call/list"));
        for rest in [
            "api/v1/packages/shopping/settings",
            "api/v1/packages/shopping/call",
            "api/v1/packages/shopping/call/list/more",
            "api/v1/packages/call/list",
            "api/v1/workshop/drafts/d1/call/list",
        ] {
            assert!(!plugin_call(rest), "{rest}");
        }
    }

    #[test]
    fn a_path_that_would_lead_out_of_the_whitelist_does_not_pass() {
        for path in [
            "api/v1/brain",
            "health",
            "api/v1/packages/calendar",
            "panel-plugins/radio/1.2.0/panel.js",
            "media/radio/обложка дня.png",
        ] {
            assert!(plain_path(path), "{path}");
        }
        for path in [
            "api/v1/../../debug/sessions",
            "api/v1/%2e%2e/%2E%2E/debug/sessions",
            "api/v1/.%2e/mcp/plugins",
            "api/v1/./brain",
            "api/v1//brain",
            "api/v1/brain/",
            "api/v1/..\\..\\debug",
            "api/v1/x%2F..%2F..%2Fdebug",
            "api/v1/brain?debug",
            "api/v1/brain#debug",
            "api/v1/.\t./debug",
            "api/v1/.. ",
            "api/v1/ ",
        ] {
            assert!(!plain_path(path), "{path}");
        }
        // What it stands against: the address of the request reads encoded dots as a step up.
        let url = reqwest::Url::parse("http://192.0.2.23:8790/api/v1/%2e%2e/%2E%2E/debug/sessions").unwrap();
        assert_eq!(url.path(), "/debug/sessions");
    }

    #[test]
    fn the_station_gives_a_person_their_own_role_a_key_a_guests_and_each_role_its_own_copies() {
        assert_eq!(helper_role(&user_principal("u1", "Аня", Role::Member, None)), Role::Member);
        assert_eq!(helper_role(&user_principal("u2", "Мама", Role::Admin, None)), Role::Admin);
        let key = Principal {
            kind: PrincipalKind::ApiKey,
            id: "k1".into(),
            name: "Скрипт".into(),
            role: None,
            grants: vec!["*".into()],
            denies: Vec::new(),
        };
        assert_eq!(helper_role(&key), Role::Guest, "a key, even with every right");
        let name = |role| copy_name(role, "api/v1/requests", Some("limit=4"));
        assert_ne!(name(Role::Admin), name(Role::Member));
        assert!(name(Role::Member).starts_with("member-"));
    }

    #[test]
    fn of_a_stations_copies_the_newest_stay() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now();
        for index in 0..5u64 {
            let path = dir.path().join(format!("member-{index}.json"));
            std::fs::write(&path, b"{}").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(100 - index)).unwrap();
        }
        std::fs::write(dir.path().join("member-9.tmp"), b"").unwrap();
        keep_newest(dir.path(), 3);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["member-2.json", "member-3.json", "member-4.json", "member-9.tmp"]);
    }

    #[test]
    fn a_stations_copies_are_its_own_folder_and_go_whole_never_more() {
        let state = tempfile::tempdir().unwrap();
        let kitchen = copies_of(Some(state.path()), "h:kitchen").unwrap();
        assert_eq!(kitchen, state.path().join("station-copies").join("h_kitchen"));
        for key in ["", ".", "..", "..."] {
            assert_eq!(copies_of(Some(state.path()), key), None, "{key:?}");
        }
        std::fs::create_dir_all(&kitchen).unwrap();
        std::fs::write(kitchen.join("member-1.json"), b"{}").unwrap();
        forget_copies(Some(state.path()), "..");
        assert!(kitchen.exists() && state.path().exists(), "a key of dots removes nothing");
        forget_copies(Some(state.path()), "h:kitchen");
        assert!(!kitchen.exists() && state.path().join("station-copies").exists());
    }

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    /// A pretend station gateway: it answers with the headers it was asked with, and notes what it was asked.
    async fn pretend_station(asked: Arc<Mutex<Vec<String>>>) -> u16 {
        let big = || async { axum::Json(json!({ "text": "a".repeat(COPY_LIMIT) })) };
        let echo = move |request: axum::extract::Request| {
            let asked = asked.clone();
            async move {
                let header = |name: &str| {
                    let value = request.headers().get(name).map(|value| value.as_bytes().to_vec());
                    String::from_utf8(value.unwrap_or_default()).unwrap_or_default()
                };
                asked.lock().unwrap().push(format!("{} {}", request.method(), request.uri().path()));
                axum::Json(json!({ "message": {
                    "role": header(HELPER_ROLE),
                    "person": header(HELPER_PERSON),
                    "key": header("authorization"),
                }}))
            }
        };
        let router = Router::new().route("/api/v1/big", axum::routing::get(big)).fallback(echo);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    fn kitchen(port: u16, settings_home: SettingsHome) -> StationPlacement {
        StationPlacement {
            placement: Placement::Station,
            address: "127.0.0.1".into(),
            port,
            device: Some("h:kitchen".into()),
            settings_home,
            auto_update: false,
            managed: false,
            home_key_id: None,
            moved_at: None,
        }
    }

    /// A request to the computer's gateway as it comes from `from`, signed with `token` (none: an empty one).
    async fn send(app: &Arc<App>, from: &str, method: &str, uri: &str, token: &str) -> (StatusCode, HeaderMap, Value) {
        let body = if method == "GET" || method == "HEAD" { String::new() } else { "{}".into() };
        let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        if !token.is_empty() {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let mut request = request.body(HttpBody::from(body)).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_pass_carries_the_persons_role_stays_at_home_and_keeps_each_roles_copies() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let port = pretend_station(asked.clone()).await;
        let state = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([
            ("STATE_DIR", state.path().to_string_lossy().into_owned()),
            ("PANEL_REMOTE", "true".to_string()),
        ]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
        let person = json!({"name": "Аня", "role": "member", "password": "member-password-1"});
        app.access.users.create(person.as_object().unwrap(), None).await.unwrap();
        let LoginResult::Ok(member) = app.access.auth.login(Some("Аня"), "member-password-1", "test").await.unwrap()
        else {
            panic!("the member signs in")
        };
        let member = member.token;
        let script = app
            .access
            .keys
            .create(
                &Map::from_iter([("name".into(), json!("Скрипт")), ("grants".into(), json!(["station.view"]))]),
                None,
            )
            .unwrap()
            .secret;
        app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
        let watch = app.station_gateway.watch("h:kitchen");
        watch.set_record(Some(&kitchen(port, SettingsHome::Station)), None).unwrap();
        let home = "192.168.1.20";
        let journal = "/station/h:kitchen/api/v1/requests?limit=4";

        // Each person reaches the station with their own role: a member stays a member there.
        let (status, _, body) = send(&app, home, "GET", journal, &member).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["message"], json!({"role": "member", "person": "Аня", "key": format!("Bearer {PAIR}")}));
        let (_, _, body) = send(&app, home, "GET", journal, &admin).await;
        assert_eq!(
            (body["message"]["role"].as_str(), body["message"]["person"].as_str()),
            (Some("admin"), Some("Мама"))
        );
        let (_, _, body) = send(&app, home, "GET", "/station/h:kitchen/api/v1/station", &script).await;
        assert_eq!(body["message"]["role"], "guest", "a key: a guest's rights there");

        // From outside the home the pass is closed, even to the admin with the panel open outside.
        let (status, _, body) = send(&app, "8.8.8.8", "GET", journal, &admin).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));

        // A path that would step out of the whitelist, and a station nobody linked, go nowhere.
        let before = asked.lock().unwrap().len();
        for uri in [
            "/station/h:kitchen/api/v1/..%2F..%2Fdebug%2Fsessions",
            "/station/h:kitchen/api/v1/%252e%252e/%252e%252e/debug/sessions",
            "/station/h:kitchen/api/v1/../../debug/sessions",
        ] {
            assert_eq!(send(&app, home, "GET", uri, &admin).await.0, StatusCode::NOT_FOUND, "{uri}");
        }
        let (status, _, body) = send(&app, home, "GET", "/station/h:nobody/api/v1/brain", &admin).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["message"], "Такой колонки у этого компьютера нет.");
        assert_eq!(asked.lock().unwrap().len(), before);

        // While its settings live here, a change made in its window would be overwritten: refused, said why.
        assert_eq!(send(&app, home, "PUT", "/station/h:kitchen/api/v1/brain", &admin).await.0, StatusCode::OK);
        watch.set_record(Some(&kitchen(port, SettingsHome::Pc)), None).unwrap();
        let (status, _, body) = send(&app, home, "PUT", "/station/h:kitchen/api/v1/brain", &admin).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["message"], crate::placement::routes::SETTINGS_ON_PC);
        assert!(body["message"].as_str().unwrap().contains("«Вернуть настройки на колонку»"));
        assert_eq!(asked.lock().unwrap().iter().filter(|asked| asked.starts_with("PUT")).count(), 1);
        assert_eq!(send(&app, home, "GET", journal, &member).await.0, StatusCode::OK, "reading goes on");
        // A plugin's settings travel too: refused. What does not travel goes on — a plugin's call (its widgets read
        // with it, a member's too: the station checks its right with the member's role), the player.
        let (settings, on_pc) =
            ("/station/h:kitchen/api/v1/packages/calendar/settings", Some(crate::placement::routes::SETTINGS_ON_PC));
        let (status, _, body) = send(&app, home, "PUT", settings, &admin).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, on_pc));
        let call = "/station/h:kitchen/api/v1/packages/calendar/call/today";
        let (status, _, body) = send(&app, home, "POST", call, &member).await;
        assert_eq!((status, body["message"]["role"].as_str()), (StatusCode::OK, Some("member")), "{body}");
        assert_eq!(send(&app, home, "POST", call, &admin).await.0, StatusCode::OK);
        let pause = "/station/h:kitchen/api/v1/station/player/pause";
        assert_eq!(send(&app, home, "POST", pause, &admin).await.0, StatusCode::OK);
        assert_eq!(send(&app, home, "POST", pause, &member).await.0, StatusCode::FORBIDDEN, "a change: the admin's");
        // Review G1: a character chosen, the journal's days, the catalog, the workshop's model travel too; the mode
        // is the station's own, and what writes nothing goes on.
        for (method, rest) in [
            ("POST", "api/v1/characters/alice/choose"),
            ("PUT", "api/v1/requests/keep"),
            ("PUT", "api/v1/packages/catalog"),
            ("PUT", "api/v1/workshop/agent"),
        ] {
            let (status, _, body) = send(&app, home, method, &format!("/station/h:kitchen/{rest}"), &admin).await;
            assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, on_pc), "{rest}");
        }
        for rest in ["api/v1/station/mode", "api/v1/brain/ask", "api/v1/connectors/weather/check"] {
            let (status, _, body) = send(&app, home, "POST", &format!("/station/h:kitchen/{rest}"), &admin).await;
            assert_eq!((status, body["message"]["role"].as_str()), (StatusCode::OK, Some("admin")), "{rest}");
        }

        // The station's plugins' panel parts and the workshop's previews come without a key (`import()`, `<link>`):
        // they pass without the session — from home, on the whitelist — and go as anyone's: no key, nobody named.
        for uri in [
            "/station/h:kitchen/panel-plugins/radio/1.3.0/panel.js",
            "/station/h:kitchen/panel-preview/t0k3n/panel.css",
        ] {
            let (status, headers, body) = send(&app, home, "GET", uri, "").await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            assert_eq!(body["message"], json!({"role": "", "person": "", "key": ""}), "{uri}");
            // The app's window (a frame of no origin) imports it across origins: the allowance passes too.
            assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*", "{uri}");
            assert_eq!(send(&app, home, "HEAD", uri, "").await.0, StatusCode::OK, "{uri}");
            assert_eq!(send(&app, "8.8.8.8", "GET", uri, "").await.0, StatusCode::FORBIDDEN, "{uri}: from outside");
            assert_eq!(send(&app, home, "POST", uri, "").await.0, StatusCode::UNAUTHORIZED, "{uri}: only a reading");
        }
        let before = asked.lock().unwrap().len();
        for uri in
            ["/station/h:kitchen/panel-plugins/../api/v1/requests", "/station/h:kitchen/panel-preview/%2e%2e/debug"]
        {
            assert_eq!(send(&app, home, "GET", uri, "").await.0, StatusCode::NOT_FOUND, "{uri}");
        }
        assert_eq!(send(&app, home, "GET", journal, "").await.0, StatusCode::UNAUTHORIZED, "the rest asks who");
        assert_eq!(asked.lock().unwrap().len(), before);
        let (_, headers, _) = send(&app, home, "GET", journal, &admin).await;
        assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN), "the API is not opened across origins");

        // A big answer is not copied.
        let (status, _, _) = send(&app, home, "GET", "/station/h:kitchen/api/v1/big", &admin).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!copy_path(&app, "h:kitchen", Role::Admin, "api/v1/big", None).unwrap().exists());

        // The station away: each role gets its own copy, nobody another's.
        watch.set_record(Some(&kitchen(9, SettingsHome::Station)), None).unwrap();
        let (status, headers, body) = send(&app, home, "GET", journal, &member).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(headers.contains_key(COPY_AT));
        assert_eq!(body["message"]["role"], "member");
        let (_, _, body) = send(&app, home, "GET", journal, &admin).await;
        assert_eq!(body["message"]["role"], "admin");
        let (status, _, _) = send(&app, home, "GET", journal, &script).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "a key never got a copy of the journal");

        // The station's record removed: what it showed through the pass goes with it.
        let copies = copies_dir(&app, "h:kitchen").unwrap();
        assert!(copies.is_dir());
        watch.set_record(None, None).unwrap();
        assert!(!copies.exists(), "the copies go with the record");
    }
}
