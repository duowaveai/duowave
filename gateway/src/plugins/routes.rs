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

use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::budget::ResourcesView;
use super::catalog::CatalogView;
use super::controller::{PackageView, PackagesView};
use super::manifest::PackageKind;
use crate::app::App;
use crate::core::events::Actor;
use crate::core::identity::permissions::use_of;
use crate::core::identity::permissions::{
    CONNECTORS_VIEW, PERSONA_EDIT, PLUGINS_MANAGE, PLUGINS_MCP, SETTINGS_EDIT, STATION_VIEW,
};
use crate::core::identity::{PrincipalKind, Role};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::capabilities::CapabilityName;

/// A body is read before the route asks who calls: kept small.
const BODY_LIMIT: usize = 64 * 1024;

pub fn routes() -> Router<Arc<App>> {
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
        .route("/api/v1/packages/{id}/schedule", get(schedule))
        .route("/api/v1/packages/{id}/schedule/{job}", put(schedule_pause).delete(schedule_remove))
        .route("/api/v1/packages/{id}/schedule/{job}/run", post(schedule_run))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// The admin, a person, at home — and not one whose right to plugins was taken away.
pub(crate) fn admin(caller: &Caller) -> Result<Option<Actor>, ApiError> {
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
        Some(PackageKind::Theme) => caller.permitted(SETTINGS_EDIT)?,
        Some(PackageKind::Plugin | PackageKind::App) | None => return admin(caller),
    };
    Ok(caller.actor())
}

fn refresh(query: Option<&str>) -> bool {
    url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .any(|(key, value)| key == "refresh" && matches!(value.as_ref(), "1" | "true" | ""))
}

/// The catalog with how each package stands here; `?refresh=1` reads the feed again now.
async fn catalog(State(app): State<Arc<App>>, caller: Caller, RawQuery(query): RawQuery) -> ApiResult<CatalogView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.catalog.view(refresh(query.as_deref())).await))
}

#[derive(Debug, Deserialize)]
pub struct CatalogSwitch {
    enabled: bool,
}

/// The owner's switch: `{enabled}`. Off, the gateway does not ask the network for the catalog.
async fn catalog_switch(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<CatalogSwitch>,
) -> ApiResult<CatalogView> {
    caller.permitted(SETTINGS_EDIT)?;
    app.catalog.set_enabled(body.enabled, caller.actor())?;
    Ok(ok(app.catalog.view(false).await))
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
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<SourceBody>,
) -> ApiResult<CatalogView> {
    let actor = admin(&caller)?;
    if !body.understood {
        return Err(ApiError::bad_request(crate::tr!(
            "Свой источник добавляется после предупреждения: его пакеты никто не проверял (understood: true)."
        )));
    }
    Ok(ok(app.catalog.add_source(&body.url, actor).await?))
}

/// An own source removed: `{url}`. What was installed from it stays.
async fn remove_source(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<SourceBody>,
) -> ApiResult<CatalogView> {
    let actor = admin(&caller)?;
    Ok(ok(app.catalog.remove_source(&body.url, actor).await?))
}

/// A package's preview from the copy here: `image`, or a phrase by its number from 1.
async fn preview(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, which)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    caller.permitted(STATION_VIEW)?;
    let (file, kind) = app.catalog.preview(&id, &which).await?;
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
}

/// From the catalog: `{version?}` — the newest without it.
async fn install(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<InstallBody>,
) -> ApiResult<PackageView> {
    let actor = may_handle(&caller, app.catalog.kind_of(&id).await)?;
    let view = app.catalog.install(&id, body.version.as_deref(), false, actor).await?;
    app.characters.sync();
    Ok(created(view))
}

/// The newest version from the catalog over the installed one.
async fn update(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    let actor = may_handle(&caller, app.catalog.kind_of(&id).await)?;
    let view = app.catalog.install(&id, None, true, actor).await?;
    app.characters.sync();
    Ok(created(view))
}

/// Every version of a package and its row; a character's voices with it.
async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    let kind = app.catalog.kind_of(&id).await;
    let actor = may_handle(&caller, kind)?;
    app.characters.before_removal(&id)?;
    // Its history lives on for 30 days, with its data as it is now (docs/plans/package-history.md).
    if let Some(history) = &app.history {
        if app.plugins.origin_of(&id).removable() {
            history.note_removed(&id, actor.as_ref());
        }
    }
    app.catalog.remove(&id, actor).await?;
    app.characters.sync();
    // Its pictures go with it (plugins/files.rs).
    app.files.forget(&id);
    Ok(ok(json!({ "deleted": true })))
}

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<PackagesView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.plugins.view().await))
}

/// The themes of the installed packages, for «Вид пульта». Whoever sees the panel picks a theme.
async fn themes(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "themes": app.plugins.themes() })))
}

/// The plugins' panel parts to load: every panel that sees the station loads them.
async fn panel_parts(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "parts": app.plugins.panel_parts() })))
}

/// A plugin's panel part calls its backend (`useApi`): whoever may use the plugins, with the function's input. The
/// backend acts in that person's chain on the bus (src/bus/trail.rs): what it reaches, it reaches as them.
async fn panel_call(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, function)): Path<(String, String)>,
    Body(input): Body<Value>,
) -> ApiResult<Value> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    let call = app.plugins.panel_call(&id, &function, input);
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
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<ForkBody>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(created(app.plugins.fork(&id, body.id.as_deref(), actor).await?))
}

/// A plugin's part of the panel: `panel.js` (an ES module the panel imports) and `panel.css`, of a plugin that is on,
/// of the version in force. Served like the panel's own files — a module script carries no key — and only these two:
/// nothing else of a package runs on the panel's origin. Its rights are its manifest's, checked by the API it calls.
async fn panel_file(
    State(app): State<Arc<App>>,
    Path((id, version, file)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let path = app.plugins.panel_file(&id, &version, &file).ok_or_else(|| ApiError::not_found("Not Found"))?;
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
    State(app): State<Arc<App>>,
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
            let tools: Vec<Value> = app
                .plugins
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
            match app.plugins.tool_contributions().into_iter().find(|tool| tool.tool.name == name) {
                Some(tool) if allowed(&tool.permission) => {
                    let context = crate::core::connectors::ToolContext {
                        principal: principal.clone(),
                        cancel: tokio_util::sync::CancellationToken::new(),
                        station: None,
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
async fn resources(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<ResourcesView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.plugins.resources().await))
}

async fn one(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    caller.permitted(CONNECTORS_VIEW)?;
    app.plugins.rescan();
    Ok(ok(app.plugins.view_one(&id).await?))
}

async fn enable(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(app.plugins.enable(&id, actor).await?))
}

async fn disable(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(app.plugins.disable(&id, actor).await?))
}

async fn reload(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(app.plugins.reload(&id, actor).await?))
}

/// Back to the version before.
async fn rollback(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(app.plugins.rollback(&id, actor).await?))
}

/// The values of the package's settings by key; null puts one back to its default.
async fn settings(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(values): Body<Map<String, Value>>,
) -> ApiResult<PackageView> {
    let actor = admin(&caller)?;
    Ok(ok(app.plugins.set_settings(&id, &values, actor).await?))
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
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(body): Body<TryBody>,
) -> ApiResult<TryAnswer> {
    admin(&caller)?;
    let input = Value::Object(body.arguments.unwrap_or_default());
    Ok(ok(TryAnswer { content: app.plugins.try_tool(&id, &name, input).await? }))
}

/// Every plugin's pictures together, and the station's limit (plugins/files.rs): «Ресурсы колонки».
async fn files_summary(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.files.summary()))
}

/// A plugin's pictures for its card: names, sizes, the limit they count against here.
async fn files_view(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    let room = app.plugins.installed(&id).map_or(1, |package| package.manifest.budget.storage_mb);
    Ok(ok(app.files.view(&id, room)))
}

/// A plugin's picture for its part of the panel (`useFile`): with the panel's key, never an open link.
async fn file(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    let (bytes, kind) = app
        .files
        .read(&id, &name)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Нет картинки {name}.", name = name)))?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    Ok(response)
}

/// The switch «Расписания плагинов» (plugins/schedule.rs): whoever sees the connectors sees it.
async fn schedules_switch(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.schedules.switch_view()))
}

#[derive(Debug, Deserialize)]
pub struct SwitchBody {
    on: bool,
}

/// Every plugin's schedule on or off — the admin's; the jobs stay either way.
async fn schedules_set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<SwitchBody>) -> ApiResult<Value> {
    admin(&caller)?;
    app.schedules.set_on(body.on);
    Ok(ok(app.schedules.switch_view()))
}

/// A plugin's schedule: its rhythms and tasks, when each comes next, how each ended the last time.
async fn schedule(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.schedules.view(&id)))
}

#[derive(Debug, Deserialize)]
pub struct PauseBody {
    paused: bool,
}

async fn schedule_pause(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
    Body(body): Body<PauseBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    app.schedules.pause(&id, &job, body.paused).map_err(ApiError::bad_request)?;
    Ok(ok(app.schedules.view(&id)))
}

/// A task the code set goes; a rhythm of the manifest is only paused.
async fn schedule_remove(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    app.schedules.remove(&id, &job).map_err(ApiError::bad_request)?;
    Ok(ok(app.schedules.view(&id)))
}

/// «Запустить сейчас»: what it did.
async fn schedule_run(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, job)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let outcome = app.schedules.run_now(&id, &job).await.map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "outcome": outcome })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::plugins::catalog::tests::{arthur, site};
    use crate::plugins::install::tests::{character_manifest, vgp};
    use axum::http::StatusCode;

    fn status<T>(result: ApiResult<T>) -> StatusCode {
        match result {
            Ok(answer) => answer.0,
            Err(error) => error.status,
        }
    }

    /// The switch «Расписания плагинов»: whoever sees the connectors sees it, only the admin turns it, and it is kept;
    /// a plugin's schedule is seen the same way, its jobs paused and run by the admin alone.
    #[tokio::test]
    async fn the_schedules_switch_and_a_plugins_schedule_are_the_admins_to_change() {
        let app = crate::app::App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        let seen = schedules_switch(State(app.clone()), member()).await.unwrap().1;
        assert_eq!(seen, json!({ "on": true, "allowed": true }));
        let refused = schedules_set(State(app.clone()), member(), Body(SwitchBody { on: false })).await;
        assert_eq!(status(refused), StatusCode::FORBIDDEN);
        let off = schedules_set(State(app.clone()), admin(), Body(SwitchBody { on: false })).await.unwrap().1;
        assert_eq!(off, json!({ "on": false, "allowed": true }));
        assert!(!app.schedules.is_on());
        // A plugin with no schedule: nothing to show, nothing to run.
        let none = schedule(State(app.clone()), member(), Path("nothing".into())).await.unwrap().1;
        assert_eq!(none, json!({ "on": false, "jobs": [] }));
        let pause = schedule_pause(
            State(app.clone()),
            member(),
            Path(("nothing".into(), "x".into())),
            Body(PauseBody { paused: true }),
        )
        .await;
        assert_eq!(status(pause), StatusCode::FORBIDDEN);
        let run = schedule_run(State(app.clone()), admin(), Path(("nothing".into(), "x".into()))).await;
        assert_eq!(status(run), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn installs_and_removes_by_the_packages_kind_and_never_code_from_an_unsigned_catalog() {
        let site = site().await;
        let mut theme = arthur("1.0.0");
        theme["id"] = json!("dusk");
        theme["kind"] = json!("theme");
        let mut plugin = arthur("1.0.0");
        // Not a built-in's id: «Радио» is laid out by the build itself.
        plugin["id"] = json!("clock");
        plugin["kind"] = json!("plugin");
        let mut dusk = character_manifest("dusk", "1.0.0");
        dusk["kind"] = json!("theme");
        dusk["contributes"] = json!({ "themes": [] });
        site.publish(&[
            (arthur("1.2.0"), vgp(&character_manifest("arthur", "1.2.0"))),
            (theme, vgp(&dusk)),
            (plugin, b"code".to_vec()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
            ("DATA_DIR", text("data")),
            ("PACKAGES_FEED_URL", site.url.clone()),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        let body = || Body(InstallBody::default());

        let listed = catalog(State(app.clone()), member(), RawQuery(None)).await.unwrap().1;
        assert_eq!(listed.packages.len(), 3);
        let installing = install(State(app.clone()), member(), Path("arthur".into()), body()).await;
        assert_eq!(status(installing), StatusCode::FORBIDDEN, "a character is who may change the character's");
        let installed = install(State(app.clone()), admin(), Path("arthur".into()), body()).await.unwrap();
        assert_eq!((installed.0, installed.1.source.as_str()), (StatusCode::CREATED, "catalog"));
        assert!(app.characters.character("arthur:arthur").is_some(), "its characters at once");
        assert_eq!(
            status(install(State(app.clone()), member(), Path("dusk".into()), body()).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(install(State(app.clone()), admin(), Path("dusk".into()), body()).await),
            StatusCode::CREATED
        );
        let unsigned = install(State(app.clone()), admin(), Path("clock".into()), body()).await;
        assert_eq!(status(unsigned), StatusCode::CONFLICT);
        assert!(!dir.path().join("packages/clock").exists());
        // The built-in «Радио» is there from the build, and it is not removed.
        assert!(dir.path().join("packages/radio").is_dir());
        assert_eq!(status(remove(State(app.clone()), admin(), Path("radio".into())).await), StatusCode::CONFLICT);

        // The owner's switch: off, nothing installs.
        let off = catalog_switch(State(app.clone()), admin(), Body(CatalogSwitch { enabled: false })).await.unwrap();
        assert!(!off.1.enabled);
        assert_eq!(status(update(State(app.clone()), admin(), Path("arthur".into())).await), StatusCode::CONFLICT);
        let switched = catalog_switch(State(app.clone()), member(), Body(CatalogSwitch { enabled: true })).await;
        assert_eq!(status(switched), StatusCode::FORBIDDEN);

        assert_eq!(status(remove(State(app.clone()), member(), Path("arthur".into())).await), StatusCode::FORBIDDEN);
        assert_eq!(status(remove(State(app.clone()), admin(), Path("arthur".into())).await), StatusCode::OK);
        assert!(app.characters.character("arthur:arthur").is_none());
        let image = preview(State(app.clone()), member(), Path(("arthur".into(), "image".into()))).await.unwrap();
        assert_eq!(image.headers()[header::CONTENT_TYPE], "image/webp");
    }
}
