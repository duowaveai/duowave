//! «Мозг» в пульте: провайдеры моделей (готовые и свои, со своими ключами) и выбор быстрой и умной модели. Видит
//! всякий, кто видит подключения; ключи не уходят ни в один ответ — только «задан · …a1b2». Добавляет и меняет
//! провайдеров человек с правом подключений, выбирает модели — с правом моделей. Действует со следующего вопроса,
//! без перезапуска. Провайдер, у сервиса которого есть вход через аккаунт (OpenRouter), получает ключ входом
//! (src/oauth): «Войти через …» начинается здесь, возврат принимает /api/v1/oauth.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::Router;
use serde_json::{json, Map, Value};

use super::brain_service::{BrainView, ProviderCheck, ProviderInput, ProviderView};
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW, MODELS_MANAGE};
use crate::core::identity::{Principal, PrincipalKind};
use crate::dialog::brain::BrainService;
use crate::oauth::{client_name, invite_code, Extra, OAuthService, SignInStart};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::secrets::SecretsService;

pub fn routes<S: crate::dialog::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/brain", get(view))
        .route("/api/v1/brain/providers", post(add))
        .route("/api/v1/brain/providers/{id}", patch(update).delete(remove))
        .route("/api/v1/brain/providers/{id}/check", post(check))
        .route("/api/v1/brain/providers/{id}/models", post(models))
        .route("/api/v1/brain/providers/{id}/signin", post(sign_in))
        .route("/api/v1/brain/choice", put(choose))
        .route("/api/v1/brain/invite", put(invite))
}

/// Keys are entered by people, never by a script's key.
fn person(principal: &Principal) -> Result<(), ApiError> {
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    Ok(())
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

pub async fn view(Uses(brain): Uses<Arc<BrainService>>, caller: Caller) -> ApiResult<BrainView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(brain.view()))
}

pub async fn add(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<ProviderView> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    Ok(created(brain.add(&ProviderInput::from_value(body), caller.actor())?))
}

async fn update(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<ProviderView> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    Ok(ok(brain.update(&id, &ProviderInput::from_value(body), caller.actor())?))
}

pub async fn remove(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    brain.remove(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn check(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<ProviderCheck> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let check = brain.check(&id, body.get("model")).await?;
    // A provider's list just arrived: a fast cloud model may now be named exactly for the default backup.
    brain.keep_backup();
    Ok(ok(check))
}

pub async fn models(
    Uses(brain): Uses<Arc<BrainService>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    match brain.models(&id).await {
        Ok(models) => {
            brain.keep_backup();
            Ok(ok(json!({"models": models})))
        }
        Err(error) => Err(match error.downcast::<ApiError>() {
            Ok(api) => api,
            // A plain failure is a 500, as Nest answered it; the log gets the reason without secrets.
            Err(error) => ApiError::internal(secrets.redact(&format!("models of {id}: {error:#}"))),
        }),
    }
}

/// «Войти через …»: the service's page for the owner. The panel open on the gateway's own computer gets the browser
/// back by itself; from another device the page shows a code to paste — a return to localhost would not reach here.
///
/// DuoWave's takes more, all optional (an empty body is as before): `invite` — the invitation's code, kept for the
/// sign-ins that follow until one brings a key; `client` — this device's name for the key in the account.
pub async fn sign_in(
    Uses(brain): Uses<Arc<BrainService>>,
    Uses(oauth): Uses<Arc<OAuthService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<SignInBody>,
) -> ApiResult<SignInStart> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    let extra =
        Extra { invite: invite_of(body.invite.as_deref())?, client: body.client.as_deref().and_then(client_name) };
    Ok(ok(brain.sign_in(&id, &oauth, caller.zone == Zone::Local, extra)?))
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct SignInBody {
    #[serde(default)]
    invite: Option<String>,
    #[serde(default)]
    client: Option<String>,
}

/// An invitation's code as given: trimmed, empty as none, one that is not a code refused in words.
fn invite_of(given: Option<&str>) -> Result<Option<String>, ApiError> {
    match given.map(str::trim).filter(|code| !code.is_empty()) {
        None => Ok(None),
        Some(code) if invite_code(code) => Ok(Some(code.to_string())),
        Some(_) => Err(ApiError::bad_request(crate::tr!("Код приглашения не похож на настоящий. Проверьте его."))),
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct InviteBody {
    #[serde(default)]
    pub code: Option<String>,
}

/// The invitation's code for DuoWave's next sign-in (`{code}`; null or empty forgets it). «Мозг» then says `invite`.
pub async fn invite(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Body(body): Body<InviteBody>,
) -> ApiResult<BrainView> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    brain.set_invite(invite_of(body.code.as_deref())?.as_deref(), caller.actor())?;
    Ok(ok(brain.view()))
}

pub async fn choose(
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<BrainView> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(ok(brain.choose(&object(body), caller.actor())?))
}
