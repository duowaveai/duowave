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
//!
//! The centre of a home this computer is the Turbo of over the link (another computer or a server; stage 7 of
//! docs/plans/home-setup-flow.md) has the same pass by its client's key, `/station/link:<id>/…` (`link_pass`): its
//! requests go back over the link that centre opened (`up`).
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::core::identity::permissions::STATION_VIEW;
use crate::core::identity::principal::PrincipalKind;
use crate::core::identity::roles::Role;
use crate::helpers::link::{plain_path, LinkError, UpPerson, BODY_MAX};
use crate::platform::http::{ApiError, Caller, Uses, Zone};
use crate::storage::Db;
use crate::utils::time::now_ms;

/// The header a copy carries: when the station answered it, unix ms.
pub const COPY_AT: &str = "x-duowave-copy-at";
/// And since when the station is away, unix ms, when its watch knows it.
pub const AWAY_SINCE: &str = "x-duowave-away-since";
/// A request that waits for the station itself (the first steps, docs/plans/home-setup-flow.md): while it is away —
/// a move's reboot, about 40 s — no copy and no queued change, but the answer below, and the panel asks again. A change
/// kept in the queue answers with nothing of the station's, and a step that goes on from it (a provider added, then its
/// sign-in) would go on from nothing: 24.11, `providers/undefined/signin`.
pub const WAIT_FOR: &str = "x-duowave-wait";
/// The mark of an answer to such a request: the station is away (not slow) — asked again in a few seconds, it may be back.
pub const AWAY: &str = "x-duowave-away";
/// Who on the computer made the change (for the station's history), and their role (for their rights there).
// A request to the station as the person at the computer: the helper's pass, the base floor's.
use crate::platform::http::peer::{as_person, helper_role};
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
pub const COPY_LIMIT: usize = 2 * 1024 * 1024;

pub fn routes<S: crate::placement::RouteNeeds>() -> Router<S> {
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
pub fn passable(rest: &str) -> bool {
    rest == "health"
        || ["api/v1/", "panel-plugins/", "panel-preview/", "media/"].iter().any(|prefix| rest.starts_with(prefix))
}

/// What the station serves without a key: its plugins' panel parts, the workshop's previews by their token, and a
/// photo, a video or a sound by the media engine's key (`media/v/<key>`, gateway/media — the key is the pass). The
/// panel's `import()`, `<link>`, `<img>` and `<video>` send no key, so these pass without the session check — from
/// home only, on the same whitelist.
pub fn public_file(rest: &str) -> bool {
    ["panel-plugins/", "panel-preview/", "media/v/", "media/folder/"].iter().any(|prefix| rest.starts_with(prefix))
}

/// A plugin's call (`api/v1/packages/<id>/call/<function>`): the plugin's widgets read their data with it, so here it
/// is a reading — the station checks the call's own right with the person's role, and the computer's sending
/// overwrites nothing of it.
pub fn plugin_call(rest: &str) -> bool {
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["api", "v1", "packages", id, "call", function] => !id.is_empty() && !function.is_empty(),
        _ => false,
    }
}

/// A phrase said to the home (docs/plans/home-and-devices.md, stage 1): the computer's microphone talks to the home's
/// centre through this pass — `api/v1/listen` (the sound, piece by piece), `api/v1/talk` and `api/v1/talk/speech` (the
/// phrase and its answer). Saying something is not a change of the station's settings: anyone at home who may see it
/// may speak, and the station checks the person's own right to talk with their role.
pub fn speaking(rest: &str) -> bool {
    matches!(rest, "api/v1/listen" | "api/v1/talk" | "api/v1/talk/speech")
}

/// What is the person's own at the centre, not the station's settings (stage 2): their desk and what the panel
/// remembers about them. Anyone at home who may see the home writes their own.
fn own_row(rest: &str) -> bool {
    matches!(rest, "api/v1/desk/gateway" | "api/v1/panel/state")
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
pub fn copies_of(state_dir: Option<&FsPath>, key: &str) -> Option<PathBuf> {
    let folder: String =
        key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    if folder.chars().all(|c| c == '.') {
        return None;
    }
    state_dir.map(|dir| dir.join("station-copies").join(folder))
}

pub fn copies_dir(config: &Arc<Config>, key: &str) -> Option<PathBuf> {
    copies_of(config.skills.state_dir.as_deref(), key)
}

/// The station's record is gone (src/placement/watch.rs): what it showed through the pass goes with it.
pub fn forget_copies(state_dir: Option<&FsPath>, key: &str) {
    if let Some(dir) = copies_of(state_dir, key).filter(|dir| dir.is_dir()) {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(target: "placement", "the station {key}'s copies stay in {}: {error}", dir.display());
        }
    }
}

/// A copy is the role's: what the station showed an admin is never shown to a member while it is away.
pub fn copy_path(config: &Arc<Config>, key: &str, role: Role, rest: &str, query: Option<&str>) -> Option<PathBuf> {
    copies_dir(config, key).map(|dir| dir.join(copy_name(role, rest, query)))
}

pub fn copy_name(role: Role, rest: &str, query: Option<&str>) -> String {
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
pub fn keep_newest(dir: &FsPath, kept: usize) {
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

/// A change kept for the centre while it is away.
fn queued(name: &str) -> String {
    crate::tr!("«{name}» сейчас не на связи: правка сохранена здесь и дойдёт до хаба, когда он вернётся.", name = name)
}

pub fn away(name: &str) -> String {
    crate::tr!("Колонка «{name}» не на связи: изменения можно сделать, когда она вернётся.", name = name)
}

/// The station's panel through this computer: the pass takes its parts — its stations, configuration (the copies'
/// folder), the database (the person behind the key) — and every part of the request, each an extractor of its own.
#[allow(clippy::too_many_arguments)]
async fn pass(
    Uses(station_gateway): Uses<Arc<super::watch::StationGateways>>,
    Uses(config): Uses<Arc<Config>>,
    Uses(db): Uses<Arc<Db>>,
    Uses(clients): Uses<Arc<crate::helpers::clients::Clients>>,
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
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
    // The centre of a home this computer is the Turbo of over the link: back over that link.
    if link_client(&key) {
        let asked = Asked { method, headers: &headers, rest: &rest, query: query.as_deref(), body };
        return link_pass(&clients, &link, &caller, &key, asked).await;
    }
    let reading = matches!(method, Method::GET | Method::HEAD);
    let waits = headers.contains_key(WAIT_FOR);
    // The station's plugins' panel parts and the workshop's previews are public there and come without a key
    // (`import()`, `<link>`): no session asked. Everything else — the person on the computer: reading needs the right
    // to see the station; a change — an admin here. On the station they have their role's rights (HELPER_ROLE), no
    // more. A plugin's call is a reading: the station checks its right with that role.
    let principal = if reading && public_file(&rest) { None } else { Some(caller.permitted(STATION_VIEW)?.clone()) };
    let changing = !reading && !(method == Method::POST && (plugin_call(&rest) || speaking(&rest))) && !own_row(&rest);
    if changing && principal.as_ref().and_then(|principal| principal.role) != Some(Role::Admin) {
        return Err(ApiError::forbidden(crate::tr!("Править настройки колонки отсюда может администратор.")));
    }
    if !plain_path(&rest) || !passable(&rest) {
        return Err(ApiError::not_found("Not Found"));
    }
    // Only a station with a placement here: an unknown key makes no watch.
    let Some(gateway) = station_gateway.find(&key) else {
        return Err(ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")));
    };
    let record = gateway.record();
    let Some(peer) = gateway.client() else {
        return Err(ApiError::conflict(crate::tr!("Колонка не связана с этим компьютером: нет ключа пары.")));
    };
    // The station as people know it: its name in this computer's list, else its address; the key last of all.
    let name = gateway
        .name()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| record.map(|record| record.address).filter(|address| !address.is_empty()))
        .unwrap_or_else(|| key.clone());
    let path = format!("/{rest}{}", query.as_deref().map(|query| format!("?{query}")).unwrap_or_default());
    // The address as it will be asked, checked once more: nothing in it may lead out of the whitelist.
    let url = reqwest::Url::parse(&format!("{}{path}", peer.base()))
        .ok()
        .filter(|url| passable(url.path().trim_start_matches('/')))
        .ok_or_else(|| ApiError::not_found("Not Found"))?;
    let role = principal.as_ref().map(helper_role);
    let copy =
        role.filter(|_| method == Method::GET).and_then(|role| copy_path(&config, &key, role, &rest, query.as_deref()));

    let mut request = client().request(method.clone(), url);
    // The panel's language too: the station answers its words in it, as to its own panel (test 19.11, remark 12: «Добавить
    // провайдера» showed the providers in English in a Russian panel — the station fell back to its own language).
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::RANGE, header::ACCEPT_LANGUAGE] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    // A public file goes as anyone's at home: no key, nobody named.
    if let Some(principal) = &principal {
        request = as_person(principal, request.bearer_auth(peer.key()));
    }
    // The change as it came: kept for the centre when it is away (src/home/queue.rs).
    let kept_body = body.clone();
    if changing || !body.is_empty() {
        request = request.body(body);
    }
    // The live feed through the pass is an older panel's: the new one hears the station on its own socket (relay.rs).
    let live_feed = rest == "api/v1/live";
    if !live_feed {
        request = request.timeout(if changing { CHANGE_WAIT } else { READ_WAIT });
    }

    // A station already away is not waited for on every request: it is tried every few seconds — by its watch's looks,
    // or by a request here when they are not going on —, and between the tries the pass answers at once (watch.rs,
    // AWAY_LOOK).
    let sent = if gateway.worth_a_try() { Some(request.send().await) } else { None };
    let response = match sent {
        Some(Ok(response)) => {
            gateway.answered();
            response
        }
        failed => {
            let error = failed.and_then(Result::err);
            // It took the request and did not answer in time: it is there, but slow — not «away».
            let slow = error.as_ref().is_some_and(|error| error.is_timeout() && !error.is_connect());
            match error.as_ref() {
                // Into the journal by the watch: one line when the link goes, one when it is back — not one a request.
                Some(error) if !slow => gateway.missed(&super::peer::describe(error), &super::peer::why(error)),
                Some(error) => {
                    tracing::info!(target: "placement", "pass to the station {key}: slow — {}", super::peer::why(error))
                }
                None => {}
            }
            // Away: the last copy of what was asked, read-only; a change waits for the station.
            if let Some(found) = copy.as_ref().filter(|_| !waits).and_then(read_copy) {
                return Ok(copy_response(found, gateway.away_since()));
            }
            if slow {
                return Err(ApiError::unavailable(crate::tr!(
                    "Колонка «{name}» не ответила вовремя — попробуйте ещё раз.",
                    name = name
                )));
            }
            // A change while the home's centre is away waits for it here (docs/plans/home-and-devices.md, stage 3): it
            // goes in order when the centre is back, under the person who made it.
            if waits {
                let mut out = ApiError::unavailable(away(&name)).into_response();
                out.headers_mut().insert(HeaderName::from_static(AWAY), HeaderValue::from_static("1"));
                return Ok(out);
            }
            if changing {
                if let (Some(principal), Some(role)) = (&principal, role) {
                    let content_type = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok());
                    let change = crate::home::queue::Change {
                        method: method.as_str(),
                        path: &path,
                        content_type,
                        body: &kept_body,
                        person: &principal.name,
                        person_id: (principal.kind == PrincipalKind::User).then_some(principal.id.as_str()),
                        role: role.as_str(),
                    };
                    return match crate::home::queue::keep(&db, &change) {
                        Ok(_) => Ok((
                            StatusCode::ACCEPTED,
                            [(header::CONTENT_TYPE, "application/json")],
                            serde_json::json!({ "message": queued(&name), "queued": true }).to_string(),
                        )
                            .into_response()),
                        Err(why) => Err(ApiError::unavailable(why)),
                    };
                }
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
    // The live feed and anything else that streams goes through as it comes; a sound or a video with what seeking
    // needs (its range and length), never whole in this computer's memory.
    let media = content_type.starts_with("audio/") || content_type.starts_with("video/");
    if live_feed || content_type.starts_with("text/event-stream") || media {
        let ranges: Vec<(header::HeaderName, HeaderValue)> =
            [header::CONTENT_RANGE, header::ACCEPT_RANGES, header::CONTENT_LENGTH]
                .into_iter()
                .filter(|_| media)
                .filter_map(|name| {
                    let value = response.headers().get(name.as_str())?.to_str().ok()?;
                    Some((name, HeaderValue::from_str(value).ok()?))
                })
                .collect();
        let stream = response.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other));
        let mut out = Response::new(Body::from_stream(stream));
        *out.status_mut() = status;
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            out.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        out.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        out.headers_mut().extend(ranges);
        return Ok(out);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ApiError::bad_gateway(crate::tr!("Колонка оборвала ответ: {error}", error = error)))?;
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

/// A key of the pass that names a home this computer helps over the link — its client's id, `link:<12 hex>` —, not a
/// station.
fn link_client(key: &str) -> bool {
    key.strip_prefix("link:").is_some_and(|id| {
        id.len() == 12 && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// A request to pass on as it came: its method, headers, path after the key, query and body.
struct Asked<'a> {
    method: Method,
    headers: &'a HeaderMap,
    rest: &'a str,
    query: Option<&'a str>,
    body: Bytes,
}

/// The pass to the centre of a home this computer is the Turbo of over the link (stage 7 of
/// docs/plans/home-setup-flow.md). Such a centre — another computer or a server — dials this computer, and this one
/// holds no address or key of it: the panel's requests go back over the link the centre opened (`up`). The station
/// pass's rules, a little narrower: from the home network only (`pass` checked it), a reading needs the right to see
/// the home and a change an admin here; a person only — a key's script is nobody at home —, named to the centre with
/// their role here; the panel's API and the centre's health only; no copies while the centre is away. The centre lets
/// in only a Turbo of its own at home and acts as that person with that role, never above it (`Helpers::takes_up`).
async fn link_pass(
    clients: &Arc<crate::helpers::clients::Clients>,
    link: &Arc<crate::helpers::link_serve::LinkServer>,
    caller: &Caller,
    client: &str,
    asked: Asked<'_>,
) -> Result<Response, ApiError> {
    let Asked { method, headers, rest, query, body } = asked;
    let reading = matches!(method, Method::GET | Method::HEAD);
    let principal = caller.permitted(STATION_VIEW)?;
    let role = match (principal.kind, principal.role) {
        (PrincipalKind::User, Some(role)) => role,
        _ => return Err(ApiError::forbidden(crate::tr!("Пульт дома отсюда открывает человек, а не ключ."))),
    };
    let changing = !reading && !(method == Method::POST && (plugin_call(rest) || speaking(rest))) && !own_row(rest);
    if changing && role != Role::Admin {
        return Err(ApiError::forbidden(crate::tr!("Править настройки дома отсюда может администратор.")));
    }
    if !plain_path(rest) || !(rest.starts_with("api/v1/") || (reading && rest == "health")) {
        return Err(ApiError::not_found("Not Found"));
    }
    let Some(record) = clients.record_of(client).filter(|record| record.confirmed) else {
        return Err(ApiError::not_found(crate::tr!("Такого дома у этого компьютера нет.")));
    };
    let name = if record.home.name.trim().is_empty() { client.to_string() } else { record.home.name };
    if body.len() > BODY_MAX {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, crate::tr!("Запрос слишком большой для хаба дома.")));
    }
    let Some(calls) = link.up_of(client) else {
        return Err(match link.takes_up(client) {
            Some(false) => ApiError::conflict(crate::tr!(
                "Хаб дома «{name}» не открывает свой пульт через этот компьютер: на нём DuoWave постарше, или этот компьютер для него не Турбо дома.",
                name = name
            )),
            _ => ApiError::unavailable(crate::tr!(
                "Хаб дома «{name}» не на связи: его пульт откроется, когда он вернётся.",
                name = name
            )),
        });
    };
    let path = format!("/{rest}{}", query.map(|query| format!("?{query}")).unwrap_or_default());
    let uri = path.parse::<axum::http::Uri>().map_err(|_| ApiError::not_found("Not Found"))?;
    let mut request = axum::extract::Request::new(Body::from(body));
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    for carried in [header::CONTENT_TYPE, header::ACCEPT, header::RANGE, header::ACCEPT_LANGUAGE] {
        if let Some(value) = headers.get(&carried) {
            request.headers_mut().insert(carried, value.clone());
        }
    }
    // Who asks: the person signed in here, with their role here — in the request's frame, never as a header.
    request.extensions_mut().insert(UpPerson {
        name: principal.name.chars().filter(|c| !c.is_control()).take(40).collect(),
        role: role.as_str().to_string(),
        id: principal.id.chars().filter(|c| c.is_ascii_graphic()).take(64).collect(),
    });
    let wait = if changing { CHANGE_WAIT } else { READ_WAIT };
    let mut response = match calls.call_within(request, wait).await {
        Ok(response) => response,
        Err(LinkError::Away) => {
            return Err(ApiError::unavailable(crate::tr!(
                "Хаб дома «{name}» не на связи: его пульт откроется, когда он вернётся.",
                name = name
            )))
        }
        Err(LinkError::Busy) => {
            return Err(ApiError::unavailable(crate::tr!("Хаб дома «{name}» занят — попробуйте ещё раз.", name = name)))
        }
        Err(LinkError::Broken(why)) => {
            tracing::info!(target: "placement", "pass to the home's centre over the link: {why}");
            return Err(ApiError::bad_gateway(crate::tr!(
                "Хаб дома «{name}» не ответил — попробуйте ещё раз.",
                name = name
            )));
        }
    };
    // A bare refusal is the centre's own door: this computer is not its Turbo at home (any more).
    if response.status() == StatusCode::FORBIDDEN && axum::body::HttpBody::is_end_stream(response.body()) {
        return Err(ApiError::forbidden(crate::tr!(
            "Хаб дома «{name}» не пускает этот компьютер к своему пульту: в его списке Турбо он не «дома».",
            name = name
        )));
    }
    // The API is not opened across origins through the pass, and the browser keeps no answer.
    let headers = response.headers_mut();
    let crossing: Vec<HeaderName> =
        headers.keys().filter(|name| name.as_str().starts_with("access-control-")).cloned().collect();
    for name in crossing {
        headers.remove(name);
    }
    headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The last copy, marked as one — and, when the station's outage is known, since when it is away (the panel: «не в
/// сети с 14:05»).
fn copy_response(copy: Copy, away_since: Option<u64>) -> Response {
    let mut out = (StatusCode::from_u16(copy.status).unwrap_or(StatusCode::OK), copy.body).into_response();
    if let Ok(value) = HeaderValue::from_str(&copy.content_type) {
        out.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&copy.at.to_string()) {
        out.headers_mut().insert(COPY_AT, value);
    }
    if let Some(value) = away_since.and_then(|since| HeaderValue::from_str(&since.to_string()).ok()) {
        out.headers_mut().insert(AWAY_SINCE, value);
    }
    out.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    out
}
