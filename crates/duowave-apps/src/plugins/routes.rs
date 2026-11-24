//! `/api/v1/packages`: the packages for the panel, of every kind. Whoever sees the connectors sees them: their
//! state, budget, rights, whether a plugin's code is loaded, its calls and last error. Switching one on or off,
//! reloading it, changing its settings and trying a plugin's tools is the admin's alone, a person at home: a plugin
//! is code that runs in the gateway with the rights its manifest asks for.
//!
//! The catalog (`/api/v1/packages/catalog`) is seen by whoever sees the station; installing, updating and removing a
//! package is by its kind: a character — who may change the character (`persona.edit`), a theme — who changes the
//! gateway's settings (`settings.edit`), a plugin or an app — the admin at home (`plugins.manage`). Own sources of
//! packages (`/api/v1/packages/sources`, docs/plans/custom-feeds.md) are added and removed by the admin at home alone.
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, RawQuery};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::budget::ResourcesView;
use super::catalog::CatalogView;
use super::controller::{PackageView, PackagesView};
use super::manifest::{PackageKind, Permission};
use crate::bus::Address;
use crate::bus::BusFolders;
use crate::capabilities::publish::Publishers;
use crate::characters::CharactersService;
use crate::core::events::Actor;
use crate::core::identity::permissions::use_of;
use crate::core::identity::permissions::{
    CONNECTORS_VIEW, PERSONA_EDIT, PLUGINS_MANAGE, PLUGINS_MCP, SETTINGS_EDIT, STATION_VIEW,
};
use crate::core::identity::{PrincipalKind, Role};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::plugins::files::PluginFiles;
use crate::plugins::history::History;
use crate::plugins::schedule::Scheduler;
use crate::plugins::watch::WatchPages;
use crate::plugins::{PackageCatalog, PluginHost};
use crate::settings::SettingsService;
use crate::skills::capabilities::CapabilityName;

/// A body is read before the route asks who calls: kept small.
const BODY_LIMIT: usize = 64 * 1024;

pub fn routes<S: crate::plugins::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/packages", get(list))
        .route("/api/v1/packages/resources", get(resources))
        .route("/api/v1/packages/themes", get(themes))
        .route("/api/v1/packages/panel", get(panel_parts))
        .route("/api/v1/packages/{id}/call/{function}", post(panel_call))
        .route("/api/v1/packages/{id}/fork", post(fork))
        .route("/panel-plugins/{id}/{version}/{file}", get(panel_file))
        .route("/mcp/plugins", post(mcp_plugins))
        .route("/api/v1/packages/catalog", get(catalog).put(catalog_switch))
        .route("/api/v1/packages/sources", post(add_source).delete(remove_source))
        .route("/api/v1/packages/catalog/{id}/preview/{which}", get(preview))
        .route("/api/v1/packages/{id}", get(one).delete(remove))
        .route("/api/v1/packages/{id}/install", post(install))
        .route("/api/v1/packages/{id}/update", post(update))
        .route("/api/v1/packages/{id}/enable", post(enable))
        .route("/api/v1/packages/{id}/disable", post(disable))
        .route("/api/v1/packages/{id}/reload", post(reload))
        .route("/api/v1/packages/{id}/rollback", post(rollback))
        .route("/api/v1/packages/{id}/settings", put(settings))
        .route("/api/v1/packages/{id}/tools/{name}/try", post(try_tool))
        .route("/api/v1/packages/schedules", get(schedules_switch).put(schedules_set))
        .route("/api/v1/packages/files", get(files_summary))
        .route("/api/v1/packages/{id}/files", get(files_view))
        .route("/api/v1/packages/{id}/files/{name}", get(file))
        .route("/api/v1/packages/{id}/share", post(share))
        .route("/mcp/apps/{id}", post(app_mcp).get(app_mcp_get))
        .route("/api/v1/packages/{id}/schedule", get(schedule))
        .route("/api/v1/packages/{id}/schedule/{job}", put(schedule_pause).delete(schedule_remove))
        .route("/api/v1/packages/{id}/schedule/{job}/run", post(schedule_run))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        // A recording from the app's window comes as WAV: a minute and more of voice is megabytes.
        .merge(
            Router::new()
                .route("/api/v1/packages/{id}/recordings", post(recording))
                .layer(DefaultBodyLimit::max(RECORDING_MAX)),
        )
}

/// A recording from an app's window at most: five minutes of 24 kHz voice.
const RECORDING_MAX: usize = 16 * 1024 * 1024;

/// The admin, a person, at home — and not one whose right to plugins was taken away.
pub fn admin(caller: &Caller) -> Result<Option<Actor>, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let principal = caller.permitted(PLUGINS_MANAGE)?;
    if principal.kind != PrincipalKind::User || principal.role != Some(Role::Admin) {
        return Err(ApiError::forbidden(crate::tr!("Приложениями управляет только администратор.")));
    }
    Ok(caller.actor())
}

/// Who may install, update or remove a package of this kind; an unknown one is the admin's.
fn may_handle(caller: &Caller, kind: Option<PackageKind>) -> Result<Option<Actor>, ApiError> {
    match kind {
        Some(PackageKind::Character) => caller.permitted(PERSONA_EDIT)?,
        Some(PackageKind::Theme | PackageKind::Agent) => caller.permitted(SETTINGS_EDIT)?,
        Some(PackageKind::Plugin | PackageKind::App) | None => return admin(caller),
    };
    Ok(caller.actor())
}

fn refresh(query: Option<&str>) -> bool {
    url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .any(|(key, value)| key == "refresh" && matches!(value.as_ref(), "1" | "true" | ""))
}

/// The catalog with how each package stands here; `?refresh=1` reads the feed again now.
pub async fn catalog(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> ApiResult<CatalogView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(catalog.view(refresh(query.as_deref())).await))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSwitch {
    pub enabled: Option<bool>,
    pub show_hidden: Option<bool>,
}

/// The owner's switches: `{enabled}` — off, the gateway does not ask the network for the catalog; `{showHidden}` —
/// the hidden developer option «Показывать скрытые пакеты». Either or both.
pub async fn catalog_switch(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    Body(body): Body<CatalogSwitch>,
) -> ApiResult<CatalogView> {
    caller.permitted(SETTINGS_EDIT)?;
    if body.enabled.is_none() && body.show_hidden.is_none() {
        return Err(ApiError::bad_request(crate::tr!("Нечего менять: enabled или showHidden.")));
    }
    if let Some(enabled) = body.enabled {
        catalog.set_enabled(enabled, caller.actor())?;
    }
    if let Some(show) = body.show_hidden {
        catalog.set_show_hidden(show, caller.actor())?;
    }
    Ok(ok(catalog.view(false).await))
}

#[derive(Debug, Deserialize)]
pub struct SourceBody {
    url: String,
    /// The panel showed the warning and the admin agreed: packages from it are checked by no one.
    #[serde(default)]
    understood: bool,
}

/// An own source of packages by its link: `{url, understood: true}` — the admin at home, after the warning.
async fn add_source(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    Body(body): Body<SourceBody>,
) -> ApiResult<CatalogView> {
    let actor = admin(&caller)?;
    if !body.understood {
        return Err(ApiError::bad_request(crate::tr!(
            "Свой источник добавляется после предупреждения: его пакеты никто не проверял (understood: true)."
        )));
    }
    Ok(ok(catalog.add_source(&body.url, actor).await?))
}

/// An own source removed: `{url}`. What was installed from it stays.
async fn remove_source(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    Body(body): Body<SourceBody>,
) -> ApiResult<CatalogView> {
    let actor = admin(&caller)?;
    Ok(ok(catalog.remove_source(&body.url, actor).await?))
}

/// A package's preview from the copy here: `image`, or a phrase by its number from 1.
pub async fn preview(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    Path((id, which)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    caller.permitted(STATION_VIEW)?;
    let (file, kind) = catalog.preview(&id, &which).await?;
    let bytes = tokio::fs::read(&file).await.map_err(|_| ApiError::not_found(crate::tr!("Нет такого превью.")))?;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(response)
}

#[derive(Debug, Default, Deserialize)]
pub struct InstallBody {
    #[serde(default)]
    version: Option<String>,
    /// From the store (MagicPlace): a plugin or an app is switched on right after it is in place.
    #[serde(default)]
    on: bool,
    /// The person approved its rights with the install: the panel asked once, before it went in, what it costs and what
    /// it may, in words. Its links are made before it first runs, so it asks nothing at first use.
    #[serde(default)]
    rights: bool,
}

/// From the catalog: `{version?, on?, rights?}` — the newest without a version; `on` — switched on as well; `rights` —
/// its rights approved with it.
pub async fn install(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(bus): Uses<Arc<crate::bus::Bus>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<InstallBody>,
) -> ApiResult<PackageView> {
    let actor = may_handle(&caller, catalog.kind_of(&id).await)?;
    let view = catalog.install(&id, body.version.as_deref(), false, actor.clone()).await?;
    if body.rights {
        approve_rights(&plugins, &bus, &caller, &id, actor.clone());
    }
    let view = if body.on { catalog.switch_on(&id, view, actor).await? } else { view };
    characters.sync();
    Ok(created(view))
}

/// The package's rights on the bus approved with its install, before it first runs (`Bus::approve_install`). Rights
/// are the house admin's at home to give, as in «Связи»: anyone else's install leaves them to the questions at first
/// use.
pub fn approve_rights(plugins: &PluginHost, bus: &crate::bus::Bus, caller: &Caller, id: &str, actor: Option<Actor>) {
    if caller.zone_at_least(Zone::Home).is_err() || caller.permitted(PLUGINS_MANAGE).is_err() {
        return;
    }
    // The version in place, switched on or not: the install approves before it switches on (`install` above), and the
    // version in force (`package`) is none until then — the «да» was dropped without a word (clean-path check 18.11,
    // items 8, 13, 23: the wake word asked for each right again after «Поставить всё»).
    let Some(package) = plugins.installed(id) else { return };
    let rights: Vec<_> = package
        .manifest
        .permissions
        .iter()
        .filter_map(|permission| match permission {
            Permission::Bus { level, pattern } => Some((*level, pattern.clone())),
            _ => None,
        })
        .collect();
    if rights.is_empty() {
        return;
    }
    let me = Address::app(id);
    match bus.approve_install(&me, &rights, actor.clone()) {
        Ok(_) => bus.changed("links", Some(&me), actor),
        Err(error) => {
            tracing::warn!(target: "plugins", "{id}: rights not approved with the install: {}", error.message)
        }
    }
}

/// The newest version from the catalog over the installed one.
pub async fn update(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<PackageView> {
    let actor = may_handle(&caller, catalog.kind_of(&id).await)?;
    let view = catalog.install(&id, None, true, actor).await?;
    characters.sync();
    Ok(created(view))
}

/// The work folders' seats (src/works, a computer's; links/http.rs gives them): an agent's card removed, the folders
/// that had it go back to the seat «Исполнитель».
pub trait AgentSeats: Send + Sync {
    /// Frees the folders `agent` had; their names, for the words.
    fn forget_agent(&self, agent: &str) -> Vec<String>;
}

/// No work folders: the station's own build.
pub struct NoSeats;

impl AgentSeats for NoSeats {
    fn forget_agent(&self, _agent: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Every version of a package and its row; a character's voices with it. A removal reaches every part a package
/// lives in, each an extractor of its own — hence the count.
#[allow(clippy::too_many_arguments)]
pub async fn remove(
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    Uses(history_service): Uses<Option<Arc<History>>>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(files_service): Uses<Arc<PluginFiles>>,
    Uses(watch): Uses<Arc<WatchPages>>,
    Uses(bus): Uses<Arc<crate::bus::Bus>>,
    Uses(seats): Uses<Arc<dyn AgentSeats>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let kind = catalog.kind_of(&id).await;
    let actor = may_handle(&caller, kind)?;
    characters.before_removal(&id)?;
    // Its history lives on for 30 days, with its data as it is now (docs/plans/package-history.md).
    if let Some(history) = &history_service {
        if plugins.origin_of(&id).removable() {
            history.note_removed(&id, actor.as_ref());
        }
    }
    catalog.remove(&id, actor).await?;
    characters.sync();
    // The models chosen for its characters' voices go too (the centre's choice, a Турбо's named one): installed again,
    // it starts afresh.
    crate::characters::forget_package(&settings, &characters, &id);
    // Its pictures go with it (plugins/files.rs), and its pages «смотри и выбирай»; the keys to its files that the
    // media engine gave it close (gateway/media).
    files_service.forget(&id);
    watch.forget(&id);
    if let Some(files) = bus.files() {
        files.media().forget_app(&id);
    }
    // An agent's card: the work folders that had it go back to the seat «Исполнитель» — the answer names them.
    if kind == Some(PackageKind::Agent) {
        let folders = seats.forget_agent(&id);
        if !folders.is_empty() {
            let said = crate::tr!(
                "Агент убран из рабочих папок: {folders}. В них теперь работает агент по умолчанию.",
                folders = folders.join(", ")
            );
            return Ok(ok(json!({ "deleted": true, "said": said, "folders": folders })));
        }
    }
    Ok(ok(json!({ "deleted": true })))
}

async fn list(Uses(plugins): Uses<Arc<PluginHost>>, caller: Caller) -> ApiResult<PackagesView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(plugins.view().await))
}

/// The themes of the installed packages, for «Вид пульта». Whoever sees the panel picks a theme.
async fn themes(Uses(plugins): Uses<Arc<PluginHost>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "themes": plugins.themes() })))
}

/// The plugins' panel parts to load: every panel that sees the station loads them.
async fn panel_parts(Uses(plugins): Uses<Arc<PluginHost>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "parts": plugins.panel_parts() })))
}

/// A plugin's panel part calls its backend (`useApi`): whoever may use the plugins, with the function's input. The
/// backend acts in that person's chain on the bus (src/bus/trail.rs): what it reaches, it reaches as them.
async fn panel_call(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path((id, function)): Path<(String, String)>,
    Body(input): Body<Value>,
) -> ApiResult<Value> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let call = plugins.panel_call(&id, &function, input);
    // Outside the home the household's folders are closed unless ticked «и вне дома».
    let trail = crate::bus::Trail::principal(&principal).from_afar(caller.zone == crate::platform::http::Zone::Remote);
    Ok(ok(json!({ "result": trail.scope(call).await? })))
}

#[derive(Debug, Default, Deserialize)]
pub struct ForkBody {
    #[serde(default)]
    id: Option<String>,
}

/// «Сделать свою копию»: `{id?}` — `my-<id>` without it.
async fn fork(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<ForkBody>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(created(plugins.fork(&id, body.id.as_deref(), actor).await?))
}

/// A plugin's part of the panel: `panel.js` (an ES module the panel imports) and `panel.css`, of a plugin that is on,
/// of the version in force. Served like the panel's own files — a module script carries no key — and only these two:
/// nothing else of a package runs on the panel's origin. Its rights are its manifest's, checked by the API it calls.
async fn panel_file(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Path((id, version, file)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let path = plugins.panel_file(&id, &version, &file).ok_or_else(|| ApiError::not_found("Not Found"))?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found("Not Found"))?;
    let kind = if file.ends_with(".js") { "text/javascript; charset=utf-8" } else { "text/css; charset=utf-8" };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // An app's window is a sandboxed frame of no origin (docs/plans/bus.md, «Окошки»): it loads its module across
    // origins. The file is the app's public panel code, no secret in it.
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    Ok(response)
}

/// The plugins' tools as the gateway's MCP server (docs/plans/plugins.md, «MCP в обе стороны»): what the plugins that
/// are on give «Мозг», for Claude Code and other agents — `tools/list` and `tools/call` over JSON-RPC, each tool with
/// the right its use needs, as for a person. The key needs `plugins.mcp`.
async fn mcp_plugins(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Body(request): Body<Value>,
) -> Result<Response, ApiError> {
    let principal = caller.permitted(PLUGINS_MCP)?.clone();
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("id").is_none() {
        return Ok(axum::http::StatusCode::ACCEPTED.into_response());
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let allowed = |permission: &str| crate::core::identity::can(Some(&principal), permission);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "duowave-plugins", "version": crate::VERSION },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = plugins
                .tool_contributions()
                .into_iter()
                .filter(|tool| allowed(&tool.permission))
                .map(|tool| {
                    json!({
                        "name": tool.tool.name,
                        "description": tool.tool.description,
                        "inputSchema": tool.tool.parameters,
                        "annotations": { "readOnlyHint": tool.read_only },
                    })
                })
                .collect();
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            match plugins.tool_contributions().into_iter().find(|tool| tool.tool.name == name) {
                Some(tool) if allowed(&tool.permission) => {
                    let context = crate::dialog::connectors::ToolContext {
                        principal: principal.clone(),
                        cancel: tokio_util::sync::CancellationToken::new(),
                        ear: None,
                        origin: None,
                    };
                    let (text, failed) = match (tool.run)(arguments, context).await {
                        Ok(text) => (text, false),
                        Err(error) => (format!("error: {error}"), true),
                    };
                    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
                }
                Some(_) => Err((-32001, crate::tr!("нет права на {name}", name = name))),
                None => Err((-32602, crate::tr!("нет инструмента {name}", name = name))),
            }
        }
        other => Err((-32601, format!("method {other} is not here"))),
    };
    let answer = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    };
    Ok(axum::Json(answer).into_response())
}

/// The plugins' memory and room: «Ресурсы колонки» and the plugins' list.
async fn resources(Uses(plugins): Uses<Arc<PluginHost>>, caller: Caller) -> ApiResult<ResourcesView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(plugins.resources().await))
}

async fn one(Uses(plugins): Uses<Arc<PluginHost>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    caller.permitted(CONNECTORS_VIEW)?;
    plugins.rescan();
    Ok(ok(plugins.view_one(&id).await?))
}

async fn enable(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(plugins.enable(&id, actor).await?))
}

async fn disable(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(plugins.disable(&id, actor).await?))
}

async fn reload(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(plugins.reload(&id, actor).await?))
}

/// Back to the version before.
async fn rollback(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(plugins.rollback(&id, actor).await?))
}

/// The values of the package's settings by key; null puts one back to its default.
async fn settings(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(values): Body<Map<String, Value>>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(plugins.set_settings(&id, &values, actor).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct TryBody {
    #[serde(default)]
    arguments: Option<Map<String, Value>>,
}

#[derive(Debug, Serialize)]
pub struct TryAnswer {
    content: String,
}

async fn try_tool(
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(body): Body<TryBody>,
) -> ApiResult<TryAnswer> {
    admin(&caller)?;
    let input = Value::Object(body.arguments.unwrap_or_default());
    Ok(ok(TryAnswer { content: plugins.try_tool(&id, &name, input).await? }))
}

/// Every plugin's pictures together, and the station's limit (plugins/files.rs): «Ресурсы колонки».
async fn files_summary(Uses(files): Uses<Arc<PluginFiles>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(files.summary()))
}

/// A plugin's pictures for its card: names, sizes, the limit they count against here.
async fn files_view(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    let room = plugins.installed(&id).map_or(1, |package| package.manifest.budget.storage_mb);
    Ok(ok(files.view(&id, room)))
}

/// A plugin's picture for its part of the panel (`useFile`): with the panel's key, never an open link; it goes out
/// through the media engine (gateway/media) — its type by its bytes, nothing a browser runs.
async fn file(
    Uses(files): Uses<Arc<PluginFiles>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let picture = files
        .picture(&id, &name)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Нет картинки {name}.", name = name)))?;
    let engine = crate::gateway::media::Allow::Media;
    crate::gateway::media::engine::serve(picture, engine, request, "private, max-age=300").await
}

#[derive(Debug, Default, Deserialize)]
pub struct ShareBody {
    /// A file of the app's folder, if a file goes out.
    #[serde(default)]
    file: Option<String>,
    /// Where it went: `videos` (saved into the household's «Видео»), `download`, `system` (the phone's menu), `copy`
    /// (the text), or a platform's upload page (`tiktok`, `reels`, `shorts`, `telegram`, `vk`).
    #[serde(rename = "where")]
    to: String,
}

/// What a person sent out of an app's window («Поделиться», docs/plans/feed.md, stage 5): the journal keeps what and
/// where; «В Видео» is the gateway's own copy into the household's folder. The person started it — no «да» is asked.
async fn share(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(publishers): Uses<Arc<Publishers>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    Uses(bus): Uses<Arc<crate::bus::Bus>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<ShareBody>,
) -> ApiResult<Value> {
    let shared = crate::capabilities::routes::SharedBody { app: Some(id), to: body.to, file: body.file };
    Ok(ok(crate::capabilities::routes::noted(&plugins, &publishers, &files, &bus, &caller, shared).await?))
}

/// A recording from an app's window (SDK `record()`, «Записать своим голосом»): the panel opened the microphone, the
/// window never did; the WAV goes into the app's folder, `recordings/<время>.wav`, and the window gets its name.
async fn recording(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    caller: Caller,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Value> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    if plugins.installed(&id).is_none() {
        return Err(ApiError::not_found(format!("Нет приложения «{id}».")));
    }
    let wav = crate::speech::audio::pcm::decode_wav(&body).map_err(|_| ApiError::bad_request("Запись — не WAV."))?;
    let seconds = wav.pcm.len() as f64 / f64::from(wav.sample_rate.max(1));
    let name = format!("recordings/{}.wav", chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"));
    let file = files.place(&id, &name).map_err(ApiError::bad_request)?;
    std::fs::write(&file, &body).map_err(ApiError::internal)?;
    plugins.note_line(&id, "info", &format!("запись своим голосом: {name}, {seconds:.1} с"));
    Ok(created(json!({ "file": name, "seconds": (seconds * 10.0).round() / 10.0 })))
}

async fn app_mcp_get() -> Response {
    (axum::http::StatusCode::METHOD_NOT_ALLOWED, "MCP приложения: только POST (JSON-RPC)").into_response()
}

/// An app as an MCP server for one's own Claude Code (docs/plans/feed.md, stage 7: the «Лента» director with one's own
/// subscription): its own tools (`contributes.tools`, each called as an agent's call — up to five minutes) and the
/// gateway's file tools over its own folder (`list_files`, `read_file`, `write_file`). The admin's panel key goes in
/// `Authorization: Bearer`; the app must be on.
async fn app_mcp(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(request): Body<Value>,
) -> Result<Response, ApiError> {
    admin(&caller)?;
    let package = plugins.package(&id).ok_or_else(|| ApiError::not_found(format!("«{id}» не включено.")))?;
    let rpc = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("id").is_none() {
        return Ok(axum::http::StatusCode::ACCEPTED.into_response());
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = |value: Value| json!({ "jsonrpc": "2.0", "id": rpc, "result": value });
    let own = &package.manifest.contributes.tools;
    let answer = match method {
        "initialize" => result(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": format!("duowave-app-{id}"), "version": crate::VERSION },
            "instructions": format!(
                "Инструменты приложения «{}» шлюза DuoWave и файлы его папки (list_files, read_file, write_file — пути от \
                 её корня). Начните с инструмента приложения, который объясняет работу, если он есть.",
                package.manifest.title
            ),
        })),
        "ping" => result(json!({})),
        "tools/list" => {
            let mut tools: Vec<Value> = own
                .iter()
                .map(|tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": tool.input }))
                .collect();
            tools.extend(super::agent_files::agent_file_tools().into_iter().map(
                |tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": tool.parameters }),
            ));
            result(json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let path = arguments.get("path").and_then(Value::as_str).unwrap_or_default();
            let outcome = match name {
                "list_files" => super::agent_files::agent_list(&files, &id, path),
                "read_file" => super::agent_files::agent_read(&files, &id, path),
                "write_file" => super::agent_files::agent_write(
                    &files,
                    &id,
                    path,
                    arguments.get("text").and_then(Value::as_str).unwrap_or_default(),
                ),
                tool => match own.iter().find(|known| known.name == tool) {
                    Some(known) => plugins
                        .call(&id, &known.handler, arguments.clone(), crate::plugins::runtime::CallKind::Agent)
                        .await
                        .map(|answer| match answer {
                            Value::String(text) => text,
                            other => other.to_string(),
                        })
                        .map_err(|error| error.to_string()),
                    None => Err(format!("нет инструмента {tool}")),
                },
            };
            if name == "write_file" && outcome.is_ok() {
                plugins.note_line(&id, "info", &format!("Claude Code: write_file {path}"));
            }
            let (text, failed) = match outcome {
                Ok(text) => (text, false),
                Err(error) => (error, true),
            };
            result(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        other => {
            json!({ "jsonrpc": "2.0", "id": rpc, "error": { "code": -32601, "message": format!("method {other} is not here") } })
        }
    };
    let mut response = axum::Json(answer).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The switch «Расписания плагинов» (plugins/schedule.rs): whoever sees the connectors sees it.
pub async fn schedules_switch(Uses(schedules): Uses<Arc<Scheduler>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(schedules.switch_view()))
}

#[derive(Debug, Deserialize)]
pub struct SwitchBody {
    pub on: bool,
}

/// Every plugin's schedule on or off — the admin's; the jobs stay either way.
pub async fn schedules_set(
    Uses(schedules): Uses<Arc<Scheduler>>,
    caller: Caller,
    Body(body): Body<SwitchBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    schedules.set_on(body.on);
    Ok(ok(schedules.switch_view()))
}

/// A plugin's schedule: its rhythms and tasks, when each comes next, how each ended the last time.
pub async fn schedule(
    Uses(schedules): Uses<Arc<Scheduler>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(schedules.view(&id)))
}

#[derive(Debug, Deserialize)]
pub struct PauseBody {
    pub paused: bool,
}

pub async fn schedule_pause(
    Uses(schedules): Uses<Arc<Scheduler>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
    Body(body): Body<PauseBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    schedules.pause(&id, &job, body.paused).map_err(ApiError::bad_request)?;
    Ok(ok(schedules.view(&id)))
}

/// A task the code set goes; a rhythm of the manifest is only paused.
async fn schedule_remove(
    Uses(schedules): Uses<Arc<Scheduler>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    schedules.remove(&id, &job).map_err(ApiError::bad_request)?;
    Ok(ok(schedules.view(&id)))
}

/// «Запустить сейчас»: what it did.
pub async fn schedule_run(
    Uses(schedules): Uses<Arc<Scheduler>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let outcome = schedules.run_now(&id, &job).await.map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "outcome": outcome })))
}
