//! The return of a sign-in to an outside service (src/oauth): the browser coming back to this computer — on the
//! gateway's own return, or on `/auth/callback` for a service that takes only 127.0.0.1 and that path (ChatGPT) — the
//! code or the address pasted into the panel from another device, and the panel's question «вошли?». A sign-in is
//! begun where its target lives: a model provider's — `POST /api/v1/brain/providers/{id}/signin`.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::page::page;
use super::{plain_token, stale, Credential, Delivered, Given, SignInStatus, Target};
use crate::core::events::Actor;
use crate::core::identity::permissions::CONNECTORS_MANAGE;
use crate::core::identity::PrincipalKind;
use crate::dialog::brain::BrainService;
use crate::oauth::OAuthService;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Provides, RouteState, Uses, Zone};

/// The return of a service that takes 127.0.0.1 with this path only (ChatGPT).
pub const LOOPBACK_PATH: &str = "/auth/callback";

/// What the sign-in routes use of the gateway: the sign-ins, and the model providers they end at.
pub fn routes<S: RouteState + Provides<Arc<OAuthService>> + Provides<Arc<BrainService>>>() -> Router<S> {
    Router::new()
        .route(super::CALLBACK_PATH, get(callback))
        .route(LOOPBACK_PATH, get(callback))
        .route("/api/v1/oauth/complete", post(complete))
        .route("/api/v1/oauth/{id}", get(status))
}

/// The return's code and the client a registration names, from its query.
fn given_of(get: impl Fn(&str) -> Option<String>, code: String) -> Given {
    Given {
        code,
        client_id: get("client_id").map(|client| client.trim().to_string()).filter(|client| !client.is_empty()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInDone {
    /// Who got what the sign-in brought: the provider's title.
    pub title: String,
    /// What is left to do, when something is («доступа пока нет»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn cancelled() -> ApiError {
    ApiError::bad_request(crate::tr!("Вход отменён на странице сервиса. Нажмите «Войти» ещё раз."))
}

/// The longest code a service hands over that we take.
const CODE_MAX: usize = 2048;

/// The browser back from the service (on this computer it lands by itself). No sign-in to the panel is asked — the
/// state of a sign-in begun from the panel is the check; a page in words answers, never the service's own text.
async fn callback(
    Uses(oauth): Uses<Arc<OAuthService>>,
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let get = |key: &str| query.iter().find(|(name, _)| name == key).map(|(_, value)| value.trim().to_string());
    let outcome = match (get("state").filter(|state| !state.is_empty()), get("code"), get("error")) {
        (Some(id), _, Some(_)) => Err(cancel(&oauth, &id)),
        (None, _, Some(_)) => Err(cancelled()),
        (Some(id), Some(code), None) if plain_token(&code, CODE_MAX) => {
            let given = given_of(get, code);
            finish(oauth, brain, id, given, None).await
        }
        _ => Err(stale()),
    };
    Ok(match outcome {
        Ok(done) => {
            let said = crate::tr!("Вход в «{title}» выполнен. Это окно можно закрыть.", title = done.title);
            page(
                StatusCode::OK,
                &match done.note {
                    Some(note) => format!("{said} {note}"),
                    None => said.to_string(),
                },
            )
        }
        Err(error) => page(error.status, &error.message),
    })
}

/// The owner said no on the service's page: the panel that waits learns it at once.
fn cancel(oauth: &OAuthService, id: &str) -> ApiError {
    let error = cancelled();
    if oauth.take(id).is_ok() {
        oauth.settle(id, &Err(error.clone()));
    }
    error
}

#[derive(Debug, Default, Deserialize)]
pub struct CompleteBody {
    /// The sign-in begun in the panel.
    #[serde(default)]
    id: Option<String>,
    /// The code the service's page showed, or the address the browser could not open after the sign-in.
    #[serde(default)]
    text: Option<String>,
}

/// From another device: the code the service's page showed, or the address of the return, pasted into the panel.
pub async fn complete(
    Uses(oauth): Uses<Arc<OAuthService>>,
    Uses(brain): Uses<Arc<BrainService>>,
    caller: Caller,
    Body(body): Body<CompleteBody>,
) -> ApiResult<SignInDone> {
    let principal = caller.permitted(CONNECTORS_MANAGE)?;
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    let text = body.text.as_deref().map(str::trim).unwrap_or_default();
    let named = body.id.map(|id| id.trim().to_string()).filter(|id| !id.is_empty());
    let missing =
        || ApiError::bad_request(crate::tr!("Вставьте код со страницы сервиса или адрес страницы после входа."));
    let (id, given) = match url::Url::parse(text) {
        Ok(url) if url.query().is_some() => {
            let get = |key: &str| url.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned());
            if get("error").is_some() {
                return Err(match get("state").or(named) {
                    Some(id) => cancel(&oauth, &id),
                    None => cancelled(),
                });
            }
            let code = get("code").map(|code| code.trim().to_string()).filter(|code| plain_token(code, CODE_MAX));
            (get("state").filter(|state| !state.is_empty()).or(named), given_of(get, code.ok_or_else(missing)?))
        }
        _ => {
            let code = Some(text.to_string()).filter(|code| plain_token(code, CODE_MAX)).ok_or_else(missing)?;
            (named, Given { code, client_id: None })
        }
    };
    let id = id.ok_or_else(stale)?;
    Ok(ok(finish(oauth, brain, id, given, caller.actor()).await?))
}

/// «Вошли?» — the panel asks while the owner signs in on the service's page.
pub async fn status(
    Uses(oauth): Uses<Arc<OAuthService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<SignInStatus> {
    caller.permitted(CONNECTORS_MANAGE)?;
    oauth.status(&id).map(ok).ok_or_else(|| ApiError::not_found(crate::tr!("Такого входа нет: он устарел.")))
}

/// Exchanges the code and gives what it brought to who began the sign-in; the panel that waits learns how it ended.
/// It goes on when the browser leaves meanwhile: a sign-in is not lost half-way.
async fn finish(
    oauth: Arc<OAuthService>,
    brain: Arc<BrainService>,
    id: String,
    given: Given,
    actor: Option<Actor>,
) -> Result<SignInDone, ApiError> {
    tokio::spawn(async move {
        let taken = oauth.take(&id)?;
        let outcome = match oauth.exchange(&taken, &given).await {
            Ok(credential) => deliver(&brain, &taken.target, credential, actor),
            Err(error) => Err(error),
        };
        oauth.settle(&id, &outcome);
        outcome.map(|done| SignInDone { title: done.title, note: done.note })
    })
    .await
    .map_err(ApiError::internal)?
}

fn deliver(
    brain: &BrainService,
    target: &Target,
    credential: Credential,
    actor: Option<Actor>,
) -> Result<Delivered, ApiError> {
    match target {
        Target::Provider { id, created_at } => brain.signed_in(id, created_at, credential, actor),
    }
}
