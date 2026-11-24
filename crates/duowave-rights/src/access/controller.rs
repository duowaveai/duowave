//! The access routes: signing in (the panel's first question, the wizard's one step, the login, who I am, my
//! password), and the household (people, their roles and restrictions, the voice of the house, keys; the catalog
//! the panel lists). The audit log's route is beside its service.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path};
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
use super::{audit_service, js_prefix, AccessService};
use crate::config::Config;
use crate::core::capabilities::CAPABILITY_NAMES;
use crate::core::identity::permissions::{PERMISSIONS, STATION_VIEW, USERS_MANAGE};
use crate::core::identity::policy::panel_permissions;
use crate::core::identity::principal::PrincipalKind;
use crate::core::identity::roles::{Role, ROLES};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Provides, RouteState, Uses, Zone};
use crate::settings::definitions::Household;

/// The access routes use the access services and the configuration (PANEL_REMOTE), nothing else of the gateway.
pub fn routes<S>() -> Router<S>
where
    S: RouteState + Provides<Arc<AccessService>> + Provides<Arc<Config>>,
{
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
    ApiError::too_many(crate::tr!("Слишком много попыток — подождите {minutes} мин.", minutes = minutes))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelState {
    configured: bool,
    accounts: usize,
    needs_code: bool,
    /// The gateway on a speaker: the panel's hint for the first-run code says where it is there — the installer showed
    /// it at its end (test 19.11, remark 19), not «logs/duowave.log» of a Mac.
    station: bool,
}

/// Show the wizard, the login screen, or straight in.
async fn state(Uses(access): Uses<Arc<AccessService>>, caller: Caller) -> ApiResult<PanelState> {
    let auth = &access.auth;
    let configured = auth.configured()?;
    Ok(ok(PanelState {
        configured,
        accounts: auth.accounts()?,
        needs_code: !configured && caller.zone != Zone::Local,
        station: cfg!(feature = "station"),
    }))
}

/// The wizard: the first password makes the admin. Only while nobody exists, only from home; from another device
/// than this computer, with the code from the gateway's log — wrong codes counted per source as passwords are.
async fn setup(Uses(access): Uses<Arc<AccessService>>, caller: Caller, Body(body): Input) -> ApiResult<SignedIn> {
    caller.zone_at_least(Zone::Home)?;
    let auth = &access.auth;
    if caller.zone != Zone::Local {
        match auth.check_setup_code(body.get("code"), &caller.source) {
            Ok(()) => {}
            Err(SetupRefused::Throttled { wait_ms }) => return Err(too_many_tries(wait_ms)),
            Err(SetupRefused::WrongCode) => {
                return Err(ApiError::forbidden(if cfg!(feature = "station") {
                    crate::tr!("Нужен код настройки — его показал установщик в конце установки на колонку.")
                } else {
                    crate::tr!("Нужен код настройки — он в журнале DuoWave (logs/duowave.log).")
                }))
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
        None => Err(ApiError::forbidden(crate::tr!("Администратор уже задан — войдите с его паролем."))),
    }
}

async fn login(
    Uses(access): Uses<Arc<AccessService>>,
    Uses(config): Uses<Arc<Config>>,
    caller: Caller,
    Body(body): Input,
) -> ApiResult<SignedIn> {
    if caller.zone == Zone::Remote && !config.gateway.panel_remote {
        return Err(ApiError::forbidden(crate::tr!("Вход вне дома выключен (PANEL_REMOTE).")));
    }
    let name = body.get("name").and_then(Value::as_str);
    let password = body.get("password").and_then(Value::as_str).unwrap_or("");
    match access.auth.login(name, password, &caller.source).await? {
        LoginResult::Ok(signed_in) => Ok(ok(*signed_in)),
        LoginResult::Throttled { wait_ms } => Err(too_many_tries(wait_ms)),
        LoginResult::Wrong => Err(ApiError::unauthorized(crate::tr!("Неверное имя или пароль."))),
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
async fn password(Uses(access): Uses<Arc<AccessService>>, caller: Caller, Body(body): Input) -> ApiResult<Value> {
    let principal = caller.signed_in()?.clone();
    access.users.change_own_password(&principal, body.get("current"), body.get("next")).await?;
    let token = access.auth.reissue(&principal.id)?.ok_or_else(|| ApiError::unauthorized("Unauthorized"))?;
    Ok(ok(json!({ "token": token })))
}

/// «Выйти везде»: all of one's sessions end, this one too.
async fn logout_everywhere(Uses(access): Uses<Arc<AccessService>>, caller: Caller) -> Result<StatusCode, ApiError> {
    let principal = caller.signed_in()?;
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    access.users.sign_out_everywhere(&principal.id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct RoleEntry {
    id: Role,
    title: String,
    description: String,
    grants: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Entry {
    id: &'static str,
    title: String,
}

#[derive(Debug, Serialize)]
struct CapabilityEntry {
    id: &'static str,
    title: String,
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
                RoleEntry {
                    id: *role,
                    title: crate::core::tx(info.title),
                    description: crate::core::tx(info.description),
                    grants: info.grants,
                }
            })
            .collect(),
        permissions: PERMISSIONS.iter().map(|(id, title)| Entry { id, title: crate::core::tx(title) }).collect(),
        capabilities: CAPABILITY_NAMES
            .iter()
            .map(|name| CapabilityEntry {
                id: name.as_str(),
                title: crate::core::tx(name.info().title),
                permission: format!("use:{}", name.as_str()),
            })
            .collect(),
    }))
}

async fn list_people(Uses(access): Uses<Arc<AccessService>>, caller: Caller) -> ApiResult<Vec<PersonView>> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(access.users.list()?))
}

async fn add_person(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Body(body): Input,
) -> ApiResult<PersonView> {
    caller.permitted(USERS_MANAGE)?;
    Ok(created(access.users.create(&body, caller.actor()).await?))
}

async fn update_person(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Input,
) -> ApiResult<PersonView> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(access.users.update(&id, &body, caller.actor()).await?))
}

async fn remove_person(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    access.users.remove(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sign_out_person(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    access.users.sign_out_everywhere(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The voice of the house while nobody is recognised: its role and restrictions.
async fn household(Uses(access): Uses<Arc<AccessService>>, caller: Caller) -> ApiResult<Household> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(access.household.view()))
}

async fn set_household(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Body(body): Input,
) -> ApiResult<Household> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(access.household.set(&body, caller.actor())?))
}

async fn api_keys(Uses(access): Uses<Arc<AccessService>>, caller: Caller) -> ApiResult<Vec<ApiKeyView>> {
    caller.permitted(USERS_MANAGE)?;
    Ok(ok(access.keys.list()?))
}

/// A key for a script or an agent; its secret is in this answer only.
async fn create_key(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Body(body): Input,
) -> ApiResult<CreatedKey> {
    caller.permitted(USERS_MANAGE)?;
    Ok(created(access.keys.create(&body, caller.actor())?))
}

async fn revoke_key(
    Uses(access): Uses<Arc<AccessService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(USERS_MANAGE)?;
    access.keys.revoke(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}
