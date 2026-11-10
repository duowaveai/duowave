//! The station for the panel and for scripts: its mode, its routes, its live sessions and their journals, speaking
//! through it. The same routes answer at /api/v1/station (the panel) and at /debug (the debug API of the docs and
//! curl; off with DEBUG_API_ENABLED=false). Who may do what is the caller's: a signed-in person or a key with the right
//! named on each route. The debug paths stay at home.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, OriginalUri, Path, Query, State};
use axum::routing::{get, post};
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};

use crate::access::audit_service::parse_int;
use crate::app::App;
use crate::core::identity::permissions::{JOURNAL_VIEW, ROUTES_EDIT, STATION_MODE, STATION_SAY, STATION_VIEW};
use crate::gateway::mode::ModeView;
use crate::gateway::observability::session_recorder::SessionEventRecord;
use crate::gateway::uniproxy::session_registry::AnySession;
use crate::gateway::uniproxy::station_session::{SessionSnapshot, StationSession};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::skill_stats::UnsupportedEntry;
use crate::skills::skills_service::{RoutesSetting, RoutesUpdate};

pub fn router() -> Router<Arc<App>> {
    let mut router = Router::new();
    for base in ["/api/v1/station", "/debug"] {
        router = router
            .route(&format!("{base}/mode"), get(mode).post(set_mode))
            .route(&format!("{base}/mode/keep"), post(keep_mode))
            .route(&format!("{base}/sessions"), get(sessions))
            .route(&format!("{base}/routes"), get(routes).post(post_routes).put(put_routes))
            .route(&format!("{base}/unsupported"), get(unsupported))
            .route(&format!("{base}/sessions/{{id}}/events"), get(events))
            .route(&format!("{base}/sessions/{{id}}/say"), post(say))
            .route(&format!("{base}/sessions/{{id}}/tts"), post(tts))
            .route(&format!("{base}/timers"), get(timers))
            .route(&format!("{base}/player"), get(player))
            .route(&format!("{base}/player/{{action}}"), post(player_command));
    }
    // Express's JSON limit, before the access checks as there: a body over 100 KB is not read, whoever sends it.
    router.layer(DefaultBodyLimit::max(crate::access::controller::BODY_LIMIT))
}

/// The /debug paths: off with DEBUG_API_ENABLED=false, and at home only, as they always were.
fn gate(app: &App, caller: &Caller, uri: &OriginalUri) -> Result<(), ApiError> {
    if !uri.path().starts_with("/debug") {
        return Ok(());
    }
    let home = matches!(caller.zone, Zone::Home | Zone::Local);
    if !app.config.gateway.debug_api || !home {
        // Nest's NotFoundException without a message.
        return Err(ApiError::not_found("Not Found"));
    }
    Ok(())
}

/// A limit as `parseInt(limit, 10) || fallback` read it.
fn limit_of(query: &HashMap<String, String>, fallback: i64) -> usize {
    let limit = query.get("limit").and_then(|limit| parse_int(limit)).filter(|&limit| limit != 0).unwrap_or(fallback);
    limit.max(1) as usize
}

/// The station's mode as the panel shows it: Яндекс, Шлюз, Свой — and its note.
async fn mode(State(app): State<Arc<App>>, caller: Caller, uri: OriginalUri) -> ApiResult<ModeView> {
    caller.permitted(STATION_VIEW)?;
    gate(&app, &caller, &uri)?;
    Ok(ok(app.modes.view()))
}

/// The Яндекс/Шлюз/Свой switch: the station reconnects into the new mode within seconds. No gateway restart.
async fn set_mode(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Body(body): Body<Value>,
) -> ApiResult<ModeView> {
    caller.permitted(STATION_MODE)?;
    gate(&app, &caller, &uri)?;
    Ok(created(app.modes.switch(body.get("mode"), caller.actor())?))
}

/// «Остаться»: the mode the gateway switched to by itself becomes the person's; it will not switch back.
async fn keep_mode(State(app): State<Arc<App>>, caller: Caller, uri: OriginalUri) -> ApiResult<ModeView> {
    caller.permitted(STATION_MODE)?;
    gate(&app, &caller, &uri)?;
    Ok(created(app.modes.keep(caller.actor())?))
}

async fn sessions(State(app): State<Arc<App>>, caller: Caller, uri: OriginalUri) -> ApiResult<Vec<SessionSnapshot>> {
    caller.permitted(STATION_VIEW)?;
    gate(&app, &caller, &uri)?;
    Ok(ok(app.sessions.list()))
}

/// The routes in force and whether they are still the seeded default.
async fn routes(State(app): State<Arc<App>>, caller: Caller, uri: OriginalUri) -> ApiResult<RoutesSetting> {
    caller.permitted(STATION_VIEW)?;
    gate(&app, &caller, &uri)?;
    Ok(ok(app.skills.routes_setting()))
}

/// New routes from now on, without a restart: the whole map (`{"routes": {"radio": "yandex", …}}`); a capability
/// missing from it goes back to ROUTE_DEFAULT. Invalid names or steps change nothing.
async fn post_routes(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Body(body): Body<Value>,
) -> ApiResult<RoutesUpdate> {
    caller.permitted(ROUTES_EDIT)?;
    Ok(created(write_routes(&app, &caller, &uri, &body)?))
}

async fn put_routes(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Body(body): Body<Value>,
) -> ApiResult<RoutesUpdate> {
    caller.permitted(ROUTES_EDIT)?;
    Ok(ok(write_routes(&app, &caller, &uri, &body)?))
}

fn write_routes(app: &App, caller: &Caller, uri: &OriginalUri, body: &Value) -> Result<RoutesUpdate, ApiError> {
    gate(app, caller, uri)?;
    let Some(routes) = body.get("routes").and_then(Value::as_object) else {
        return Err(ApiError::bad_request("body: {\"routes\": {\"radio\": \"own|yandex|off|own,yandex\", …}}"));
    };
    let routes: BTreeMap<String, String> =
        routes.iter().map(|(name, route)| (name.clone(), crate::skills::js::string_of(route))).collect();
    let result = app.skills.replace_routes(&routes, caller.actor())?;
    if !result.problems.is_empty() {
        return Err(ApiError::bad_request(result.problems.join("; ")));
    }
    Ok(result)
}

/// The latest requests nobody could serve, newest first: the queue of what to build next. Holds what was said.
async fn unsupported(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Vec<UnsupportedEntry>> {
    caller.permitted(JOURNAL_VIEW)?;
    gate(&app, &caller, &uri)?;
    Ok(ok(app.skills.stats.recent_unsupported(limit_of(&query, 50))))
}

/// A session's journal: every frame and decision, recognised speech included.
async fn events(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Vec<SessionEventRecord>> {
    caller.permitted(JOURNAL_VIEW)?;
    gate(&app, &caller, &uri)?;
    let session = app.sessions.find(&id).ok_or_else(|| ApiError::not_found(format!("session {id} not found")))?;
    Ok(ok(session.recent_events(limit_of(&query, 100))))
}

#[derive(Serialize)]
struct Said {
    outcome: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimersView {
    device: Option<String>,
    timers: Vec<crate::skills::own::timers_skill::TimerView>,
}

/// The device a widget means when it names none (the helper's pass does not): the one heard from last, else the
/// station on the line now, else one with timers of ours — after a restart the states the requests carried are gone
/// until the next one.
fn widget_device(app: &App, query: &HashMap<String, String>) -> Option<String> {
    let live = || app.sessions.speaking_on("*").map(|session| session.device_tag.clone());
    app.skills.device_for_panel(query.get("device").map(String::as_str), live, crate::utils::time::now_ms())
}

/// The station's timers for the panel's widget: ours with their names, and the station's own it reported.
async fn timers(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<TimersView> {
    gate(&app, &caller, &uri)?;
    caller.permitted(STATION_VIEW)?;
    let device = widget_device(&app, &query);
    let timers = device.as_deref().map(|device| app.skills.timer_views(device, crate::utils::time::now_ms())).unwrap_or_default();
    Ok(ok(TimersView { device, timers }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerView {
    device: Option<String>,
    #[serde(flatten)]
    now: crate::skills::skills_service::NowPlaying,
}

/// What plays on the station, for the panel's widget.
async fn player(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<PlayerView> {
    gate(&app, &caller, &uri)?;
    caller.permitted(STATION_VIEW)?;
    let device = widget_device(&app, &query);
    let now = match device.as_deref() {
        Some(device) => app.skills.now_playing(device, crate::utils::time::now_ms()),
        None => app.skills.now_playing("", crate::utils::time::now_ms()),
    };
    Ok(ok(PlayerView { device, now }))
}

#[derive(Serialize)]
struct Pushed {
    pushed: usize,
}

/// The player from the panel: pause, resume, next, previous, stop, like — pushed to the station's live session as
/// the voice commands would answer them. What Yandex plays while its session is up is refused with the words to say
/// (the voice command goes to Yandex; the panel cannot send it there).
async fn player_command(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Path(action): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Pushed> {
    gate(&app, &caller, &uri)?;
    caller.permitted(STATION_SAY)?;
    if !matches!(action.as_str(), "pause" | "resume" | "next" | "previous" | "stop" | "like" | "dislike") {
        return Err(ApiError::bad_request("action: pause, resume, next, previous, stop, like или dislike"));
    }
    let device = app.skills.device_or_latest(query.get("device").map(String::as_str)).unwrap_or_else(|| "*".into());
    let Some(session) = app.sessions.speaking_on(&device) else {
        return Err(ApiError::conflict("Колонка не на связи."));
    };
    let yandex = app.modes.yandex_allowed() && session.yandex_connected();
    let directives = app.skills.player_command(&session.device_tag, &action, yandex).await?;
    let pushed = directives.len();
    for directive in directives {
        session.push(directive);
    }
    Ok(ok(Pushed { pushed }))
}

/// Speaks `text` on the station; `latest` picks the station's main connection.
async fn say(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Said> {
    caller.permitted(STATION_SAY)?;
    let (session, text) = speaking(&app, &caller, &uri, &id, &body)?;
    Ok(created(Said { outcome: session.say(text, None).await.as_str() }))
}

/// Synthesizes `text` through Yandex without playing it: checks the speech session silently.
async fn tts(
    State(app): State<Arc<App>>,
    caller: Caller,
    uri: OriginalUri,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(STATION_SAY)?;
    let (session, text) = speaking(&app, &caller, &uri, &id, &body)?;
    Ok(created(match session.synthesize_only(&text).await {
        Ok(check) => serde_json::to_value(check).map_err(ApiError::from)?,
        Err(error) => json!({ "error": error.to_string() }),
    }))
}

fn speaking(
    app: &App,
    caller: &Caller,
    uri: &OriginalUri,
    id: &str,
    body: &Value,
) -> Result<(Arc<StationSession>, String), ApiError> {
    gate(app, caller, uri)?;
    let text = body.get("text").and_then(Value::as_str).filter(|text| !crate::skills::js::trim(text).is_empty());
    let Some(text) = text else { return Err(ApiError::bad_request("body: {\"text\": \"...\"}")) };
    if text.encode_utf16().count() > 1000 {
        return Err(ApiError::bad_request("text: до 1000 символов"));
    }
    let session =
        if id == "latest" { app.sessions.latest_speaking().map(AnySession::Station) } else { app.sessions.find(id) };
    let Some(session) = session else {
        return Err(ApiError::not_found(if id == "latest" {
            "no live station session with credentials".to_string()
        } else {
            format!("session {id} not found")
        }));
    };
    let AnySession::Station(session) = session else {
        return Err(ApiError::bad_request("relay mode: the station talks to Yandex itself"));
    };
    Ok((session, crate::skills::js::trim(text).to_string()))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::app::App;
    use crate::config::{Config, Env};

    /// Express read no JSON body over 100 KB, and stopped before any guard: nobody signed in makes the gateway read
    /// and parse more than that.
    #[tokio::test]
    async fn reads_no_bigger_body_than_express_did_before_the_access_checks() {
        let env = Env::from_pairs([("STATE_DIR", ""), ("DEBUG_API_ENABLED", "true")]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let status = |path: &str, body: String| {
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(HttpBody::from(body))
                .unwrap();
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
            let app = app.clone();
            async move { crate::app::router(app).oneshot(request).await.unwrap().status() }
        };
        let big = format!("[{}0]", "0,".repeat(60 * 1024));
        for path in ["/api/v1/station/mode", "/api/v1/station/routes", "/debug/sessions/latest/say"] {
            assert_eq!(status(path, big.clone()).await, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        }
        assert_eq!(status("/api/v1/station/mode", "{}".into()).await, StatusCode::UNAUTHORIZED, "a small one is read");
    }
}
