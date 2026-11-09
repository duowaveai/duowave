//! The access routes: signing in (the panel's first question, the wizard's one step, the login, who I am, my
//! password), and the household (people, their roles and restrictions, the voice of the house, keys; the catalog
//! the panel lists). The audit log's route is beside its service.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::api_keys_service::{ApiKeyView, CreatedKey};
use super::auth_service::{LoginResult, SetupRefused, SignedIn};
use super::passwords::password_problem;
use super::users_repository::PersonView;
use super::users_service::MAX_NAME;
use super::{audit_service, js_prefix};
use crate::app::App;
use crate::core::identity::permissions::{PERMISSIONS, STATION_VIEW, USERS_MANAGE};
use crate::core::identity::policy::panel_permissions;
use crate::core::identity::principal::PrincipalKind;
use crate::core::identity::roles::{Role, ROLES};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::settings::definitions::Household;
use crate::skills::capabilities::CAPABILITY_NAMES;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/auth/state", get(state))
        .route("/api/v1/auth/setup", post(setup))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/password", put(password))
        .route("/api/v1/auth/logout-everywhere", post(logout_everywhere))
        .route("/api/v1/access/catalog", get(catalog))
        .route("/api/v1/users", get(list_people).post(add_person))
        .route("/api/v1/users/{id}", patch(update_person).delete(remove_person))
        .route("/api/v1/users/{id}/logout-everywhere", post(sign_out_person))
        .route("/api/v1/household", get(household).put(set_household))
        .route("/api/v1/api-keys", get(api_keys).post(create_key))
        .route("/api/v1/api-keys/{id}", delete(revoke_key))
        .merge(audit_service::routes())
        // Express's JSON limit: nothing here needs more, and the sign-in routes are open to anyone at home.
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// The largest body the access routes read (Express's default for JSON).
pub const BODY_LIMIT: usize = 100 * 1024;

type Input = Body<Map<String, Value>>;

/// «Too many tries»: in minutes, as the panel shows it.
fn too_many_tries(wait_ms: u64) -> ApiError {
    let minutes = wait_ms.div_ceil(60_000).max(1);
    ApiError::too_many(format!("Слишком много попыток — подождите {minutes} мин."))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelState {
    configured: bool,
    accounts: usize,
    needs_code: bool,
}

/// Show the wizard, the login screen, or straight in.
async fn state(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<PanelState> {
    let auth = &app.access.auth;
    let configured = auth.configured()?;
    Ok(ok(PanelState { configured, accounts: auth.accounts()?, needs_code: !configured && caller.zone != Zone::Local }))
}

/// The wizard: the first password makes the admin. Only while nobody exists, only from home; from another device
/// than this computer, with the code from the gateway's log — wrong codes counted per source as passwords are.
async fn setup(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<SignedIn> {
    caller.zone_at_least(Zone::Home)?;
    let auth = &app.access.auth;
    if caller.zone != Zone::Local {
        match auth.check_setup_code(body.get("code"), &caller.source) {
            Ok(()) => {}
            Err(SetupRefused::Throttled { wait_ms }) => return Err(too_many_tries(wait_ms)),
            Err(SetupRefused::WrongCode) => {
                return Err(ApiError::forbidden("Нужен код настройки — он в журнале шлюза (logs/duowave.log)."))
            }
        }
    }
    if let Some(problem) = password_problem(body.get("password")) {
        return Err(ApiError::bad_request(problem));
    }
    let name = match body.get("name").and_then(Value::as_str).map(str::trim) {
        Some(name) if super::js_len(name) >= 2 => js_prefix(name, MAX_NAME),
        _ => "admin",
    };
    let password = body.get("password").and_then(Value::as_str).unwrap_or_default();
    match auth.setup(password, name, &caller.source).await? {
        Some(signed_in) => Ok(created(signed_in)),
        None => Err(ApiError::forbidden("Администратор уже задан — войдите с его паролем.")),
    }
}

async fn login(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<SignedIn> {
    if caller.zone == Zone::Remote && !app.config.gateway.panel_remote {
        return Err(ApiError::forbidden("Вход вне дома выключен (PANEL_REMOTE)."));
    }
    let name = body.get("name").and_then(Value::as_str);
    let password = body.get("password").and_then(Value::as_str).unwrap_or("");
    match app.access.auth.login(name, password, &caller.source).await? {
        LoginResult::Ok(signed_in) => Ok(ok(*signed_in)),
        LoginResult::Throttled { wait_ms } => Err(too_many_tries(wait_ms)),
        LoginResult::Wrong => Err(ApiError::unauthorized("Неверное имя или пароль.")),
    }
}

#[derive(Debug, Serialize)]
struct Me {
    id: String,
    kind: PrincipalKind,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<Role>,
    permissions: Vec<String>,
}

/// Who this device is, and what it may do: the panel hides what the role cannot touch.
async fn me(caller: Caller) -> ApiResult<Me> {
    let principal = caller.signed_in()?;
    Ok(ok(Me {
        id: principal.id.clone(),
        kind: principal.kind,
        name: principal.name.clone(),
        role: principal.role,
        permissions: panel_permissions(principal),
    }))
}

/// A new password for oneself; the other sessions end, this one gets a fresh token.
async fn password(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<Value> {
    let principal = caller.signed_in()?.clone();
    app.access.users.change_own_password(&principal, body.get("current"), body.get("next")).await?;
    let token = app.access.auth.reissue(&principal.id)?.ok_or_else(|| ApiError::unauthorized("Unauthorized"))?;
    Ok(ok(json!({ "token": token })))
}

/// «Выйти везде»: all of one's sessions end, this one too.
async fn logout_everywhere(State(app): State<Arc<App>>, caller: Caller) -> Result<StatusCode, ApiError> {
    let principal = caller.signed_in()?;
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden("Это для людей, не для ключей."));
    }
    app.access.users.sign_out_everywhere(&principal.id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct RoleEntry {
    id: Role,
    title: &'static str,
    description: &'static str,
    grants: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Entry {
    id: &'static str,
    title: &'static str,
}

#[derive(Debug, Serialize)]
struct CapabilityEntry {
    id: &'static str,
    title: &'static str,
    permission: String,
}

#[derive(Debug, Serialize)]
struct Catalog {
    roles: Vec<RoleEntry>,
    permissions: Vec<Entry>,
    capabilities: Vec<CapabilityEntry>,
}

/// Roles, rights and capabilities: what the panel's editors choose from.
async fn catalog(caller: Caller) -> ApiResult<Catalog> {
    caller.signed_in()?;
    Ok(ok(Catalog {
        roles: ROLES
            .iter()
            .map(|role| {
                let info = role.info();
                RoleEntry { id: *role, title: info.title, description: info.description, grants: info.grants }
            })
            .collect(),
        permissions: PERMISSIONS.iter().map(|(id, title)| Entry { id, title }).collect(),
        capabilities: CAPABILITY_NAMES
            .iter()
            .map(|name| CapabilityEntry {
                id: name.as_str(),
                title: name.info().title,
                permission: format!("use:{}", name.as_str()),
            })
            .collect(),
    }))
}

async fn list_people(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<PersonView>> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(app.access.users.list()?))
}

async fn add_person(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<PersonView> {
    caller.permitted(USERS_MANAGE)?;
    Ok(created(app.access.users.create(&body, caller.actor()).await?))
}

async fn update_person(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Input,
) -> ApiResult<PersonView> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(app.access.users.update(&id, &body, caller.actor()).await?))
}

async fn remove_person(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    app.access.users.remove(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sign_out_person(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    app.access.users.sign_out_everywhere(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The voice of the house while nobody is recognised: its role and restrictions.
async fn household(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Household> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.access.household.view()))
}

async fn set_household(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<Household> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(app.access.household.set(&body, caller.actor())?))
}

async fn api_keys(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<ApiKeyView>> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(app.access.keys.list()?))
}

/// A key for a script or an agent; its secret is in this answer only.
async fn create_key(State(app): State<Arc<App>>, caller: Caller, Body(body): Input) -> ApiResult<CreatedKey> {
    caller.permitted(USERS_MANAGE)?;
    Ok(created(app.access.keys.create(&body, caller.actor())?))
}

async fn revoke_key(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    app.access.keys.revoke(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, Env};

    /// A request as it comes over the network from `from`.
    async fn call(app: &Arc<App>, from: &str, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        send(app, from, method, uri, body.to_string(), None).await
    }

    /// The same, with a raw body and the token it is signed with, if any.
    async fn send(
        app: &Arc<App>,
        from: &str,
        method: &str,
        uri: &str,
        body: String,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let mut request = request.body(HttpBody::from(body)).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn memory_gateway() -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap()
    }

    #[tokio::test]
    async fn counts_the_guesses_of_the_setup_code_and_makes_their_source_wait() {
        let app = memory_gateway();
        let guess = json!({"password": "admin-password-1", "code": "1000-1000"});
        for _ in 0..5 {
            let (status, _) = call(&app, "192.168.1.20", "POST", "/api/v1/auth/setup", guess.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        let right = json!({"password": "admin-password-1", "code": app.access.auth.setup_code()});
        let (status, body) = call(&app, "192.168.1.20", "POST", "/api/v1/auth/setup", right.clone()).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::TOO_MANY_REQUESTS, Some("Слишком много попыток — подождите 15 мин."))
        );
        let (status, _) = call(&app, "192.168.1.21", "POST", "/api/v1/auth/setup", right).await;
        assert_eq!(status, StatusCode::CREATED, "another device of the house is not the guesser");
    }

    #[tokio::test]
    async fn reads_no_bigger_body_than_express_did() {
        let app = memory_gateway();
        let name = "Я".repeat(BODY_LIMIT);
        let body = json!({"name": name, "password": "wrong-password"}).to_string();
        let (status, _) = send(&app, "127.0.0.1", "POST", "/api/v1/auth/login", body, None).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        let (status, _) = call(&app, "127.0.0.1", "POST", "/api/v1/auth/login", json!({"password": "x"})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a small one is read");
    }

    #[tokio::test]
    async fn a_sign_in_the_database_cannot_check_is_the_servers_error_not_sign_in_again() {
        let app = memory_gateway();
        let signed_in = app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
        let token = Some(signed_in.token.as_str());
        app.db.with(|c| c.execute_batch("DROP TABLE user_settings; DROP TABLE users;")).unwrap();
        let (status, body) = send(&app, "127.0.0.1", "GET", "/api/v1/auth/me", String::new(), token).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::INTERNAL_SERVER_ERROR, Some("Internal server error"))
        );
        let (status, _) = send(&app, "127.0.0.1", "GET", "/api/v1/household", String::new(), token).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "a right is not checked either");
        // A public route takes it for nobody and answers.
        let (status, _) = send(&app, "8.8.8.8", "GET", "/health", String::new(), token).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_first_run_from_another_device_at_home_needs_the_code_from_the_log() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let phone = "192.168.1.20";
        let (_, state) = call(&app, phone, "GET", "/api/v1/auth/state", Value::Null).await;
        assert_eq!(state["message"], json!({"configured": false, "accounts": 0, "needsCode": true}));
        let (status, body) =
            call(&app, phone, "POST", "/api/v1/auth/setup", json!({"password": "admin-password-1"})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["message"], "Нужен код настройки — он в журнале шлюза (logs/duowave.log).");
        let (status, body) = call(&app, "8.8.8.8", "POST", "/api/v1/auth/setup", json!({"password": "x"})).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));

        let code = app.access.auth.setup_code();
        let setup = json!({"password": "admin-password-1", "name": "  Мама  ", "code": code});
        let (status, body) = call(&app, phone, "POST", "/api/v1/auth/setup", setup).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["message"]["user"]["name"], "Мама");
        let (_, state) = call(&app, phone, "GET", "/api/v1/auth/state", Value::Null).await;
        assert_eq!(state["message"], json!({"configured": true, "accounts": 1, "needsCode": false}));

        // From outside the home the panel answers only with PANEL_REMOTE; its first question is open.
        let (status, body) = call(&app, "8.8.8.8", "POST", "/api/v1/auth/login", json!({"password": "x"})).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Вход вне дома выключен (PANEL_REMOTE)."))
        );
        assert_eq!(call(&app, "8.8.8.8", "GET", "/api/v1/auth/state", Value::Null).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn opens_the_panel_from_outside_when_the_owner_allows_it() {
        let env = Env::from_pairs([("STATE_DIR", ""), ("PANEL_REMOTE", "true")]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        let signed_in = app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
        let mut request = Request::builder()
            .uri("/api/v1/auth/me")
            .header("authorization", format!("Bearer {}", signed_in.token))
            .body(HttpBody::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("8.8.8.8".parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let (status, _) =
            call(&app, "8.8.8.8", "POST", "/api/v1/auth/login", json!({"password": "admin-password-1"})).await;
        assert_eq!(status, StatusCode::OK, "the login answers from outside too");
    }
}
