//! MCP-серверы для пульта. Видит всякий, кто видит подключения (секреты — только «задан · …a1b2»); добавляет,
//! меняет, включает инструменты и пробует их человек с правом подключений и только из дома: сервер — это
//! программа на этом компьютере или доступ к чужим данным.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::mcp_import::{present, Json};
use super::mcp_service::{
    ImportDone, LoginDone, LoginStart, McpServerInput, McpServerView, McpView, ToolChange, TryResult,
};
use crate::app::App;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::core::identity::{Principal, PrincipalKind};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/mcp", get(view))
        .route("/api/v1/mcp/servers", post(add))
        .route("/api/v1/mcp/import", post(import))
        .route("/api/v1/mcp/servers/{id}", patch(update).delete(remove))
        .route("/api/v1/mcp/servers/{id}/tools/{name}", put(set_tool))
        .route("/api/v1/mcp/servers/{id}/instructions", post(approve))
        .route("/api/v1/mcp/servers/{id}/reconnect", post(reconnect))
        .route("/api/v1/mcp/servers/{id}/tools/{name}/try", post(try_tool))
        .route("/api/v1/mcp/servers/{id}/login", post(login))
        .route("/api/v1/mcp/oauth/callback", get(callback))
        .route("/api/v1/mcp/oauth/complete", post(complete))
}

/// Servers are added by people, never by a script's key: a program on this computer is someone's decision.
fn person(principal: &Principal) -> Result<(), ApiError> {
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    Ok(())
}

/// From home, with the right of connectors, a person.
fn manager(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    person(caller.permitted(CONNECTORS_MANAGE)?)
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<McpView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.mcp.view()))
}

async fn add(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(input): Body<McpServerInput>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(created(app.mcp.add(&input, caller.actor())?))
}

#[derive(Debug, Default, Deserialize)]
pub struct ImportBody {
    #[serde(default, deserialize_with = "present")]
    json: Option<Json>,
}

async fn import(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<ImportBody>) -> ApiResult<ImportDone> {
    manager(&caller)?;
    Ok(created(app.mcp.import_json(body.json.as_ref(), caller.actor())?))
}

async fn update(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(input): Body<McpServerInput>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(app.mcp.update(&id, &input, caller.actor())?))
}

async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    manager(&caller)?;
    app.mcp.remove(&id, caller.actor()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_tool(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(change): Body<ToolChange>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(app.mcp.set_tool(&id, &name, &change, caller.actor())?))
}

async fn approve(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(app.mcp.approve_instructions(&id, caller.actor())?))
}

async fn reconnect(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<McpServerView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    Ok(ok(app.mcp.reconnect(&id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct TryBody {
    #[serde(default)]
    arguments: Option<Value>,
}

async fn try_tool(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(body): Body<TryBody>,
) -> ApiResult<TryResult> {
    manager(&caller)?;
    Ok(ok(app.mcp.try_tool(&id, &name, body.arguments.as_ref()).await?))
}

/// «Войти» on a server in the cloud: the page of the service for the owner's browser.
async fn login(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<LoginStart> {
    manager(&caller)?;
    Ok(ok(app.mcp.login(&id).await?))
}

/// The service sends the browser back here after the sign-in (to localhost: on this computer it lands by itself).
/// No sign-in to the panel is asked — the state of a sign-in begun from the panel is the check; a page in words
/// answers, never the service's own error text.
async fn callback(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let (status, text) = match app.mcp.complete_login(&query, None).await {
        Ok(done) => (
            StatusCode::OK,
            crate::tr!(
                "Вход в «{title}» выполнен. Это окно можно закрыть: сервер уже подключается.",
                title = done.title
            ),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, error.message),
    };
    Ok(crate::oauth::page::page(status, &text))
}

#[derive(Debug, Default, Deserialize)]
pub struct CompleteBody {
    #[serde(default, deserialize_with = "present")]
    url: Option<Json>,
}

/// From a phone: the address the browser could not open after the sign-in, pasted into the panel.
async fn complete(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<CompleteBody>) -> ApiResult<LoginDone> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    let text = body.url.as_ref().and_then(Json::as_str).map(str::trim).unwrap_or_default();
    let url =
        url::Url::parse(text).map_err(|_| ApiError::bad_request(crate::tr!("Это не адрес страницы после входа.")))?;
    let query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    Ok(ok(app.mcp.complete_login(&query, caller.actor()).await?))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    #[cfg(feature = "pc")]
    use std::time::Duration;

    use axum::extract::ConnectInfo;
    use http_body_util::BodyExt;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, user_principal};
    use crate::core::identity::Role;
    use crate::mcp::fixtures;
    use crate::mcp::mcp_service::tests::{until, READY};
    use crate::mcp::mcp_service::ServerState;
    use axum::response::IntoResponse;

    // The Node e2e signed people in over HTTP; access is ported on its own (the Rust stub signs nobody in), so the
    // handlers here are called as the guard would let these people through.

    fn app(dir: &std::path::Path) -> Arc<App> {
        let env = Env::from_pairs([("STATE_DIR", dir.to_str().unwrap())]);
        App::build(Config::for_tests(&env)).unwrap()
    }

    fn admin(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u1", "admin", Role::Admin, None)))
    }

    fn member(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)))
    }

    fn body<T: DeserializeOwned>(value: serde_json::Value) -> Body<T> {
        Body(serde_json::from_value(value).unwrap())
    }

    async fn answer(response: impl IntoResponse) -> (StatusCode, serde_json::Value) {
        let response = response.into_response();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn ready(app: &Arc<App>, id: &str) -> bool {
        app.mcp.states().iter().any(|server| server.id == id && server.state == ServerState::Ready)
    }

    #[cfg(feature = "pc")]
    fn alive(pid: i32) -> bool {
        crate::mcp::mcp_service::tests::alive(pid)
    }

    #[cfg(feature = "pc")]
    async fn add_program(app: &Arc<App>) -> serde_json::Value {
        let (command, args) = fixtures::program(fixtures::LEGACY_PROGRAM);
        let given = json!({
            "transport": "stdio",
            "title": "Fixture",
            "command": command,
            "args": args,
            "env": { "FIXTURE_VAR": "program-secret-4242" },
        });
        let (status, added) = answer(add(State(app.clone()), admin(app), body(given)).await).await;
        assert_eq!(status, StatusCode::CREATED);
        added["message"].clone()
    }

    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn adds_a_program_with_a_sealed_variable_shows_it_among_the_connectors_never_shows_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let added = add_program(&app).await;
        assert_eq!(added["id"], json!("fixture"));
        assert_eq!(added["transport"], json!("stdio"));
        assert_eq!(added["secrets"], json!([{ "kind": "env", "name": "FIXTURE_VAR", "set": true, "hint": "…4242" }]));
        until("the fixture is up", || ready(&app, "fixture"), READY).await;
        let connector = app.mcp.states().into_iter().find(|server| server.id == "fixture").unwrap();
        assert_eq!((connector.title.as_str(), connector.cloud, connector.tools), ("Fixture", true, 0));
        let (status, shown) = answer(view(State(app.clone()), admin(&app)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(shown["message"]["servers"][0]["state"], json!("ready"));
        assert!(!shown.to_string().contains("program-secret"));
        assert!(!serde_json::to_string(&app.mcp.states()).unwrap().contains("program-secret"));
        // Among the connectors as the gateway lists them, over its own routes; the value on none of them.
        let router = crate::app::router(app.clone());
        let token = sign_in(&app).await;
        let (status, listed) =
            send(&router, "GET", "/api/v1/connectors", [127, 0, 0, 1], Some(&token), "".into()).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
        let entry = listed["message"]["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["manifest"]["id"] == json!("mcp:fixture"))
            .cloned()
            .expect("the server among the connectors");
        let manifest = &entry["manifest"];
        assert_eq!(
            (&manifest["kind"], &manifest["title"], &manifest["cloud"]),
            (&json!("mcp"), &json!("Fixture"), &json!(true))
        );
        assert_eq!(entry["status"], json!({ "state": "ready", "detail": "инструментов включено: 0" }));
        for route in ["/api/v1/mcp", "/api/v1/connectors", "/health"] {
            let (status, text) = send(&router, "GET", route, [127, 0, 0, 1], Some(&token), "".into()).await;
            assert_eq!(status, StatusCode::OK, "{route}: {text}");
            assert!(!text.contains("program-secret"), "{route}: {text}");
        }
        let pid: i32 = std::fs::read_to_string(dir.path().join("mcp").join("fixture").join("fixture.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(alive(pid));
        app.shutdown().await;
    }

    #[tokio::test]
    async fn lets_the_household_see_the_servers_but_not_add_change_or_try_them() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let (router, _) = fixtures::legacy_http();
        let base = fixtures::spawn(router).await;
        let given = json!({ "transport": "http", "title": "Fixture", "url": format!("{base}/mcp") });
        let (status, _) = answer(add(State(app.clone()), admin(&app), body(given)).await).await;
        assert_eq!(status, StatusCode::CREATED);
        until("the fixture is up", || ready(&app, "fixture"), READY).await;

        assert_eq!(answer(view(State(app.clone()), member(&app)).await).await.0, StatusCode::OK);
        let program = body(json!({ "transport": "stdio", "command": "echo" }));
        assert_eq!(answer(add(State(app.clone()), member(&app), program).await).await.0, StatusCode::FORBIDDEN);
        let tool = Path(("fixture".to_string(), "echo".to_string()));
        let on = body(json!({ "on": true }));
        assert_eq!(answer(set_tool(State(app.clone()), member(&app), tool, on).await).await.0, StatusCode::FORBIDDEN);
        let tool = Path(("fixture".to_string(), "echo".to_string()));
        assert_eq!(
            answer(try_tool(State(app.clone()), member(&app), tool, body(json!({}))).await).await.0,
            StatusCode::FORBIDDEN
        );
        // A key may hold every right and still adds nothing: a program is someone's decision.
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".into()])));
        let program = body(json!({ "transport": "stdio", "command": "echo" }));
        let refused = add(State(app.clone()), key, program).await.err().unwrap();
        assert_eq!(refused.message, "Это для людей, не для ключей.");

        let tool = Path(("fixture".to_string(), "echo".to_string()));
        let tried = answer(
            try_tool(State(app.clone()), admin(&app), tool, body(json!({ "arguments": { "text": "да" } }))).await,
        )
        .await;
        assert_eq!(tried.0, StatusCode::OK);
        assert_eq!(tried.1["message"]["content"], json!("echo: да"));
        assert_eq!(tried.1["message"]["isError"], json!(false));
        app.shutdown().await;
    }

    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn stops_the_program_when_the_gateway_stops() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        add_program(&app).await;
        until("the fixture is up", || ready(&app, "fixture"), READY).await;
        let pid: i32 = std::fs::read_to_string(dir.path().join("mcp").join("fixture").join("fixture.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(alive(pid));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(crate::app::serve(app.clone(), listener, async move {
            let _ = stopped.await;
        }));
        let _ = stop.send(());
        serving.await.unwrap().unwrap();
        until("the program outlived the gateway", || !alive(pid), Duration::from_secs(5)).await;
    }

    async fn request(router: &Router, method: &str, uri: &str, from: [u8; 4]) -> (StatusCode, String) {
        send(router, method, uri, from, None, String::new()).await
    }

    /// A request as it comes over the network from `from`, signed with the token when there is one.
    async fn send(
        router: &Router,
        method: &str,
        uri: &str,
        from: [u8; 4],
        token: Option<&str>,
        body: String,
    ) -> (StatusCode, String) {
        let mut request = axum::http::Request::builder().method(method).uri(uri);
        if !body.is_empty() {
            request = request.header("content-type", "application/json");
        }
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let mut request = request.body(axum::body::Body::from(body)).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((from, 5000))));
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The first admin, signed in as the panel's wizard makes one; the token.
    #[cfg(feature = "pc")]
    async fn sign_in(app: &Arc<App>) -> String {
        app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap().token
    }

    #[tokio::test]
    async fn reads_no_bigger_body_than_express_did_before_it_asks_who_calls() {
        let dir = tempfile::tempdir().unwrap();
        let router = crate::app::router(app(dir.path()));
        // An object of many keys in a loose field, from someone who has not signed in: refused at the size, as
        // Express refused it, before anything parses it (axum alone would read up to 2 MB).
        let keys: Vec<String> = (0..20_000).map(|n| format!("\"{n:06}\":0")).collect();
        let big = format!("{{\"question\":{{{}}}}}", keys.join(","));
        assert!(big.len() > crate::mcp::BODY_LIMIT && big.len() < 2 * 1024 * 1024);
        for uri in ["/api/v1/brain/ask", "/api/v1/mcp/servers", "/api/v1/mcp/import", "/api/v1/mcp/oauth/complete"] {
            let (status, text) = send(&router, "POST", uri, [127, 0, 0, 1], None, big.clone()).await;
            assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}: {text}");
        }
        let (status, text) = send(&router, "PATCH", "/api/v1/mcp/servers/x", [127, 0, 0, 1], None, big.clone()).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
        let small = json!({ "question": "Ты здесь?" }).to_string();
        let (status, text) = send(&router, "POST", "/api/v1/brain/ask", [127, 0, 0, 1], None, small).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a small one is read: {text}");
    }

    #[tokio::test]
    async fn answers_the_panel_after_sign_in_only_and_the_services_return_with_a_page() {
        let dir = tempfile::tempdir().unwrap();
        let router = crate::app::router(app(dir.path()));
        let (status, text) = request(&router, "GET", "/api/v1/mcp", [127, 0, 0, 1]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(text.contains("Нужно войти."), "{text}");
        assert_eq!(
            request(&router, "DELETE", "/api/v1/mcp/servers/x", [127, 0, 0, 1]).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(request(&router, "POST", "/api/v1/brain/ask", [127, 0, 0, 1]).await.0, StatusCode::UNAUTHORIZED);
        let (status, page) = request(&router, "GET", "/api/v1/mcp/oauth/callback?state=x&code=y", [127, 0, 0, 1]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.starts_with("<!doctype html>") && page.contains("Этот вход устарел или начат не здесь"), "{page}");
        let (status, text) = request(&router, "GET", "/api/v1/mcp/oauth/callback?state=x", [8, 8, 8, 8]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(text.contains("Только из домашней сети."), "{text}");
    }
}
