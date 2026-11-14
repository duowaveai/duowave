//! `/api/v1/folders`: the home folder for the panel (docs/plans/shared-folders.md, «Пульт — «Папки»»). A person sees
//! the folders their role sees; the admin also where each is and changes their ticks. Attaching a folder of the disk —
//! picked in the system's window, typed, or dropped on the app's window — is the admin's, from this computer only:
//! only it knows its disk. A drive offered is opened by anyone of the family (a press here, or «да» to the station).
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{delete, get, post, put};
use axum::Router;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::drives::Offer;
use super::path;
use super::places::{Kind, ATTACHED, CONTENT_FOLDERS, PROJECTS};
use super::rules::{Change, Folders, How, Rule};
use super::Files;
use crate::app::App;
use crate::bus::address::Address;
use crate::bus::Level;
use crate::core::identity::permissions::{use_of, PLUGINS_MANAGE};
use crate::core::identity::principal::Principal;
use crate::core::identity::roles::Role;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::capabilities::CapabilityName;

/// The system's window to pick a folder: the desktop app's (tauri-plugin-dialog), set once at its start. Its title
/// in; the folder picked out, none when the person closed the window.
pub type Picker = Arc<dyn Fn(String) -> BoxFuture<'static, Option<PathBuf>> + Send + Sync>;

static PICKER: OnceLock<Picker> = OnceLock::new();

/// The desktop app gives its window to pick a folder.
pub fn set_picker(picker: Picker) {
    let _ = PICKER.set(picker);
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/folders", get(view))
        .route("/api/v1/folders/attach", post(attach))
        .route("/api/v1/folders/pick", post(pick))
        .route("/api/v1/folders/offers/{id}/look", post(look))
        .route("/api/v1/folders/offers/{id}/skip", post(skip))
        .route("/api/v1/folders/attached/{name}", delete(detach))
        .route("/api/v1/folders/rules", put(change))
        .route("/api/v1/folders/reveal", post(reveal))
        .route("/api/v1/folders/browse", get(browse))
        .route("/api/v1/folders/preview", get(preview))
        .route("/api/v1/folders/size", get(size))
}

/// The person's own place on the bus: what the panel reads, it reads as them — their role, their home or not.
fn person_letter(caller: &Caller) -> Result<(crate::bus::Trail, Address), ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let from = Address::new(crate::bus::address::PERSON, &principal.id).map_err(ApiError::bad_request)?;
    let trail = crate::bus::Trail::principal(&principal).from_afar(caller.zone == Zone::Remote);
    Ok((trail, from))
}

fn bus_error(error: crate::bus::BusError) -> ApiError {
    match error {
        crate::bus::BusError::NotFound(why) => ApiError::not_found(why),
        crate::bus::BusError::Denied(why) => ApiError::forbidden(why),
        other => ApiError::bad_request(other.to_string()),
    }
}

#[derive(Deserialize)]
struct BrowseQuery {
    address: String,
    #[serde(default)]
    from: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

/// A folder's list for the panel, a page at a time, read as the person.
async fn browse(State(app): State<Arc<App>>, caller: Caller, Query(query): Query<BrowseQuery>) -> ApiResult<Value> {
    let (trail, from) = person_letter(&caller)?;
    let to = Address::parse(&query.address).map_err(ApiError::bad_request)?;
    let input = json!({ "from": query.from, "limit": query.limit });
    app.bus.call(trail, from, to, "list", input, vec![]).await.map(ok).map_err(bus_error)
}

#[derive(Deserialize)]
struct PreviewQuery {
    address: String,
    #[serde(default)]
    size: Option<u32>,
}

/// A picture of a folder made small, for the panel's own view of the folder (as the person).
async fn preview(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<PreviewQuery>,
) -> Result<axum::response::Response, ApiError> {
    let (trail, from) = person_letter(&caller)?;
    let to = Address::parse(&query.address).map_err(ApiError::bad_request)?;
    let input = json!({ "size": query.size.unwrap_or(super::preview::SIDE) });
    let shown = app.bus.call(trail, from, to, "preview", input, vec![]).await.map_err(bus_error)?;
    jpeg(&shown)
}

/// A preview's answer as the picture itself: the panel's `<img>` takes it as a blob.
pub fn jpeg(shown: &Value) -> Result<axum::response::Response, ApiError> {
    use axum::http::{header, HeaderValue};
    use axum::response::IntoResponse;
    use base64::Engine as _;
    let data = shown.get("base64").and_then(Value::as_str).unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(ApiError::internal)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    Ok(response)
}

#[derive(Deserialize)]
struct SizeQuery {
    address: String,
}

/// How many files a folder holds and how big it is — counted up to 50 000 entries and 5 seconds.
async fn size(State(app): State<Arc<App>>, caller: Caller, Query(query): Query<SizeQuery>) -> ApiResult<Value> {
    caller.permitted(PLUGINS_MANAGE)?;
    let files = files(&app)?;
    let address = Address::parse(&query.address).map_err(ApiError::bad_request)?;
    let spot = files.places().spot(&address).ok_or_else(|| ApiError::not_found(crate::tr!("нет такой папки")))?;
    let root =
        path::existing(&spot.root, &spot.rest).map_err(|_| ApiError::not_found(crate::tr!("этой папки сейчас нет")))?;
    let counted = tokio::task::spawn_blocking(move || count(&root)).await.map_err(ApiError::internal)?;
    Ok(ok(json!({ "files": counted.0, "bytes": counted.1, "partial": counted.2 })))
}

/// Files and bytes below a folder, links not followed; stops at its limits (`partial`).
fn count(root: &std::path::Path) -> (u64, u64, bool) {
    let began = std::time::Instant::now();
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if files >= 50_000 || began.elapsed() > std::time::Duration::from_secs(5) {
                return (files, bytes, true);
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if path::hidden(&name) {
                continue;
            }
            match entry.path().symlink_metadata() {
                Ok(meta) if meta.is_dir() => queue.push(entry.path()),
                Ok(meta) if meta.is_file() => {
                    files += 1;
                    bytes += meta.len();
                }
                _ => {}
            }
        }
    }
    (files, bytes, false)
}

fn files(app: &App) -> Result<Arc<Files>, ApiError> {
    app.bus.files().ok_or_else(|| ApiError::unavailable(crate::tr!("папок здесь нет")))
}

fn folders(files: &Files) -> Result<Arc<Folders>, ApiError> {
    files.folders().ok_or_else(|| ApiError::unavailable(crate::tr!("папок здесь нет")))
}

/// The role a person sees folders with: a key and the gateway see what the admin sees.
fn role_of(principal: &Principal) -> Role {
    principal.role.unwrap_or(Role::Admin)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppReach {
    id: String,
    title: String,
    level: Level,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderView {
    address: Address,
    name: String,
    /// `home`, `content` (music, tales), `attached`, `project`.
    kind: &'static str,
    here: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    rule: Rule,
    /// The apps that reach it and how far (the admin's view).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    apps: Vec<AppReach>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct View {
    /// Where the home folder is (the admin's view).
    #[serde(skip_serializing_if = "Option::is_none")]
    home: Option<String>,
    /// The home folder is a person's own folder, seen in Finder.
    visible: bool,
    /// Something may be attached here (a computer's gateway with FOLDERS on).
    attach: bool,
    /// The system's window to pick a folder is here (the desktop app).
    pick: bool,
    /// The request comes from this computer: «Показать в Finder» and attaching work.
    local: bool,
    /// What leaves the house now: Yandex's voice says our answers; the model of «Мозг» is in the cloud.
    yandex_voice: bool,
    cloud_model: bool,
    folders: Vec<FolderView>,
    offers: Vec<Offer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

/// The apps that reach a folder: each installed app's reach on the bus, links and older rights both.
fn reach(app: &App, folder: &Address) -> Vec<AppReach> {
    app.plugins
        .installed_all()
        .iter()
        .filter_map(|package| {
            let id = package.manifest.id.clone();
            let level = app.bus.reach(&Address::app(&id), folder);
            (level > Level::None).then(|| AppReach { id, title: package.manifest.title.clone(), level })
        })
        .collect()
}

/// Every folder of the home folder a person of `role` sees: the household's (those made at the start and any a
/// person made in Finder), the music and tales, what is attached, the projects.
fn folder_views(app: &App, files: &Files, role: Role, admin: bool) -> Vec<FolderView> {
    let places = files.places();
    let Some(folders) = files.folders() else { return Vec::new() };
    let mut views = Vec::new();
    let mut add = |address: Address, kind: &'static str| {
        let Some(spot) = places.spot(&address) else { return };
        let rule = folders.rule(&spot.folder);
        if !rule.sees(role) {
            return;
        }
        let here = path::root(&spot.root).is_ok();
        let path = admin.then(|| spot.root.to_string_lossy().to_string());
        let rule = if admin { rule } else { Rule { path: None, volume: None, ..rule } };
        let apps = if admin { reach(app, &spot.folder) } else { Vec::new() };
        views.push(FolderView { name: rule.name(), address: spot.folder, kind, here, path, rule, apps });
    };
    let mut names: Vec<String> = std::fs::read_dir(&places.home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            !path::hidden(name)
                && name != ATTACHED
                && name != PROJECTS
                && !CONTENT_FOLDERS.iter().any(|(title, _)| title == name)
        })
        .collect();
    names.sort();
    for name in names {
        if let Ok(address) = Address::parse(&format!("files/home/{name}")) {
            add(address, "home");
        }
    }
    for (title, _) in CONTENT_FOLDERS {
        if let Ok(address) = Address::parse(&format!("files/home/{title}")) {
            add(address, "content");
        }
    }
    for rule in folders.attached() {
        add(rule.address.clone(), "attached");
    }
    let mut projects: Vec<String> = std::fs::read_dir(places.projects())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !path::hidden(name))
        .collect();
    projects.sort();
    for name in projects {
        if let Ok(address) = Address::parse(&format!("files/project/{name}")) {
            add(address, "project");
        }
    }
    views
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<View> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let files = files(&app)?;
    let admin = crate::core::identity::can(Some(&principal), PLUGINS_MANAGE);
    let local = caller.zone == Zone::Local;
    let remote = caller.zone == Zone::Remote;
    let mut folders = folder_views(&app, &files, role_of(&principal), admin);
    if remote {
        folders.retain(|folder| folder.rule.remote);
    }
    let (yandex_voice, cloud_model) = files.leaving();
    Ok(ok(View {
        yandex_voice,
        cloud_model,
        home: admin.then(|| files.places().home.to_string_lossy().to_string()),
        visible: files.places().visible(),
        attach: files.drives().is_some(),
        pick: PICKER.get().is_some() && local,
        local,
        folders,
        offers: if remote { Vec::new() } else { files.drives().map(|drives| drives.offers()).unwrap_or_default() },
        notes: if admin { files.places().notes() } else { Vec::new() },
    }))
}

/// The admin, from this computer: only it knows its disk.
fn admin_here(caller: &Caller) -> Result<Principal, ApiError> {
    let principal = caller.permitted(PLUGINS_MANAGE)?.clone();
    caller.zone_at_least(Zone::Local)?;
    Ok(principal)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachBody {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

async fn attach(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<AttachBody>) -> ApiResult<Rule> {
    let principal = admin_here(&caller)?;
    let files = files(&app)?;
    files
        .drives()
        .ok_or_else(|| ApiError::forbidden(crate::tr!("здесь ничего не подключают (FOLDERS=0 или колонка)")))?;
    let folders = folders(&files)?;
    let path = PathBuf::from(body.path.trim());
    folders
        .attach(&path, body.name.as_deref(), How::Picked, None, Some(&principal.id))
        .map(ok)
        .map_err(ApiError::bad_request)
}

async fn pick(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    let principal = admin_here(&caller)?;
    let files = files(&app)?;
    files
        .drives()
        .ok_or_else(|| ApiError::forbidden(crate::tr!("здесь ничего не подключают (FOLDERS=0 или колонка)")))?;
    let picker = PICKER
        .get()
        .cloned()
        .ok_or_else(|| ApiError::conflict(crate::tr!("окна выбора здесь нет: впишите путь к папке")))?;
    let Some(path) = picker(crate::tr!("Подключить папку к DuoWave")).await else {
        return Ok(ok(json!({ "cancelled": true })));
    };
    let rule =
        folders(&files)?.attach(&path, None, How::Picked, None, Some(&principal.id)).map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "rule": rule })))
}

/// Who may answer an offer: a drive — anyone of the family at home; a folder of the disk — the admin here.
fn may_answer(caller: &Caller, offer: &Offer) -> Result<Principal, ApiError> {
    match offer.kind {
        super::drives::OfferKind::Drive => {
            let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
            caller.zone_at_least(Zone::Home)?;
            if principal.role == Some(Role::Guest) {
                return Err(ApiError::forbidden(crate::tr!("гостю флешку не открыть")));
            }
            Ok(principal)
        }
        super::drives::OfferKind::Dropped => admin_here(caller),
    }
}

async fn look(State(app): State<Arc<App>>, caller: Caller, UrlPath(id): UrlPath<String>) -> ApiResult<Rule> {
    let drives = files(&app)?.drives().ok_or_else(|| ApiError::not_found(crate::tr!("предложений здесь нет")))?;
    let offer = drives.offer(&id).ok_or_else(|| ApiError::not_found(crate::tr!("этого предложения уже нет")))?;
    let principal = may_answer(&caller, &offer)?;
    drives.look(&id, Some(&principal.id)).map(ok).map_err(ApiError::bad_request)
}

async fn skip(State(app): State<Arc<App>>, caller: Caller, UrlPath(id): UrlPath<String>) -> ApiResult<Value> {
    let drives = files(&app)?.drives().ok_or_else(|| ApiError::not_found(crate::tr!("предложений здесь нет")))?;
    let offer = drives.offer(&id).ok_or_else(|| ApiError::not_found(crate::tr!("этого предложения уже нет")))?;
    may_answer(&caller, &offer)?;
    drives.skip(&id).map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "skipped": id })))
}

async fn detach(State(app): State<Arc<App>>, caller: Caller, UrlPath(name): UrlPath<String>) -> ApiResult<Value> {
    caller.permitted(PLUGINS_MANAGE)?;
    caller.zone_at_least(Zone::Home)?;
    folders(&*files(&app)?)?.detach(&name).map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "detached": name })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeBody {
    address: String,
    #[serde(flatten)]
    change: Change,
}

async fn change(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<ChangeBody>) -> ApiResult<Rule> {
    caller.permitted(PLUGINS_MANAGE)?;
    caller.zone_at_least(Zone::Home)?;
    let address = Address::parse(&body.address).map_err(ApiError::bad_request)?;
    folders(&*files(&app)?)?.change(&address, &body.change).map(ok).map_err(ApiError::bad_request)
}

#[derive(Deserialize)]
struct RevealBody {
    address: String,
}

/// «Показать в Finder»: the folder opens in the system's file manager on this computer.
async fn reveal(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<RevealBody>) -> ApiResult<Value> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    caller.zone_at_least(Zone::Local)?;
    let files = files(&app)?;
    let address = Address::parse(&body.address).map_err(ApiError::bad_request)?;
    let spot = files.places().spot(&address).ok_or_else(|| ApiError::not_found(crate::tr!("нет такой папки")))?;
    if matches!(spot.kind, Kind::App | Kind::Person) {
        return Err(ApiError::forbidden(crate::tr!("эту папку не показать")));
    }
    if let Some(folders) = files.folders() {
        if !folders.rule(&spot.folder).sees(role_of(&principal)) {
            return Err(ApiError::not_found(crate::tr!("нет такой папки")));
        }
    }
    let shown =
        path::existing(&spot.root, &spot.rest).map_err(|_| ApiError::not_found(crate::tr!("этой папки сейчас нет")))?;
    open_in_files(&shown).map_err(|error| ApiError::internal(crate::tr!("не открылась: {error}", error = error)))?;
    Ok(ok(json!({ "shown": address })))
}

/// The system's file manager at a folder: Finder, Explorer, the desktop's own on Linux.
fn open_in_files(path: &std::path::Path) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().map(|_| ())
}
