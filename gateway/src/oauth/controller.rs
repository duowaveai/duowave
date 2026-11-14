//! The return of a sign-in to an outside service (src/oauth): the browser coming back to this computer — on the
//! gateway's own return, or on `/auth/callback` for a service that takes only 127.0.0.1 and that path (ChatGPT) — the
//! code or the address pasted into the panel from another device, and the panel's question «вошли?». A sign-in is
//! begun where its target lives: a model provider's — `POST /api/v1/brain/providers/{id}/signin`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::page::page;
use super::{plain_token, stale, Credential, Given, SignInStatus, Target};
use crate::app::App;
use crate::core::events::Actor;
use crate::core::identity::permissions::CONNECTORS_MANAGE;
use crate::core::identity::PrincipalKind;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};

/// The return of a service that takes 127.0.0.1 with this path only (ChatGPT).
pub const LOOPBACK_PATH: &str = "/auth/callback";

pub fn routes() -> Router<Arc<App>> {
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
}

fn cancelled() -> ApiError {
    ApiError::bad_request(crate::tr!("Вход отменён на странице сервиса. Нажмите «Войти» ещё раз."))
}

/// The longest code a service hands over that we take.
const CODE_MAX: usize = 2048;

/// The browser back from the service (on this computer it lands by itself). No sign-in to the panel is asked — the
/// state of a sign-in begun from the panel is the check; a page in words answers, never the service's own text.
async fn callback(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let get = |key: &str| query.iter().find(|(name, _)| name == key).map(|(_, value)| value.trim().to_string());
    let outcome = match (get("state").filter(|state| !state.is_empty()), get("code"), get("error")) {
        (Some(id), _, Some(_)) => Err(cancel(&app, &id)),
        (None, _, Some(_)) => Err(cancelled()),
        (Some(id), Some(code), None) if plain_token(&code, CODE_MAX) => {
            let given = given_of(get, code);
            finish(app, id, given, None).await
        }
        _ => Err(stale()),
    };
    Ok(match outcome {
        Ok(done) => {
            page(StatusCode::OK, &crate::tr!("Вход в «{title}» выполнен. Это окно можно закрыть.", title = done.title))
        }
        Err(error) => page(error.status, &error.message),
    })
}

/// The owner said no on the service's page: the panel that waits learns it at once.
fn cancel(app: &App, id: &str) -> ApiError {
    let error = cancelled();
    if app.oauth.take(id).is_ok() {
        app.oauth.settle(id, &Err(error.clone()));
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
async fn complete(
    State(app): State<Arc<App>>,
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
                    Some(id) => cancel(&app, &id),
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
    Ok(ok(finish(app.clone(), id, given, caller.actor()).await?))
}

/// «Вошли?» — the panel asks while the owner signs in on the service's page.
async fn status(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<SignInStatus> {
    caller.permitted(CONNECTORS_MANAGE)?;
    app.oauth.status(&id).map(ok).ok_or_else(|| ApiError::not_found(crate::tr!("Такого входа нет: он устарел.")))
}

/// Exchanges the code and gives what it brought to who began the sign-in; the panel that waits learns how it ended.
/// It goes on when the browser leaves meanwhile: a sign-in is not lost half-way.
async fn finish(app: Arc<App>, id: String, given: Given, actor: Option<Actor>) -> Result<SignInDone, ApiError> {
    tokio::spawn(async move {
        let taken = app.oauth.take(&id)?;
        let outcome = match app.oauth.exchange(&taken, &given).await {
            Ok(credential) => deliver(&app, &taken.target, credential, actor),
            Err(error) => Err(error),
        };
        app.oauth.settle(&id, &outcome);
        outcome.map(|title| SignInDone { title })
    })
    .await
    .map_err(ApiError::internal)?
}

fn deliver(app: &App, target: &Target, credential: Credential, actor: Option<Actor>) -> Result<String, ApiError> {
    match target {
        Target::Provider { id, created_at } => app.brain.signed_in(id, created_at, credential, actor),
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::extract::ConnectInfo;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, user_principal};
    use crate::core::identity::Role;
    use crate::dialog::brain::brain_service::ProviderInput;
    use crate::oauth::tests::{param, service, shown, SERVICE_KEY};
    use crate::oauth::{tokens, Flow, SignIn, TokenRecord};

    fn app() -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", ""), ("DIALOG_PROVIDER", "none")]))).unwrap()
    }

    fn admin(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u-admin", "Админ", Role::Admin, None)))
    }

    /// A provider of «Мозг» without a key, as the panel adds one before its sign-in.
    fn provider(app: &Arc<App>) -> Target {
        let added = app.brain.add(&ProviderInput::from_value(json!({"preset": "openrouter"})), None).unwrap();
        assert!(!added.key.set);
        app.brain.sign_in_target(&added.id).unwrap()
    }

    /// A request as it comes over the network from `from`.
    async fn request(app: &Arc<App>, method: &str, uri: &str, from: [u8; 4]) -> (StatusCode, String) {
        let mut request =
            axum::http::Request::builder().method(method).uri(uri).body(axum::body::Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((from, 5000))));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn body(text: Value) -> Body<CompleteBody> {
        Body(serde_json::from_value(text).unwrap())
    }

    #[tokio::test]
    async fn the_browser_back_on_this_computer_brings_the_key_to_the_provider_once() {
        let (service, sign_in) = service().await;
        let app = app();
        let target = provider(&app);
        let started = app.oauth.start(&sign_in, target, true).unwrap();
        shown(&service, &started.url);

        let (status, page) = request(
            &app,
            "GET",
            &format!("/api/v1/oauth/callback?code=good-code&state={}", started.id),
            [127, 0, 0, 1],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.starts_with("<!doctype html>") && page.contains("Вход в «OpenRouter» выполнен"), "{page}");
        assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));
        let view = serde_json::to_string(&app.brain.view()).unwrap();
        assert!(!view.contains(SERVICE_KEY) && view.contains("…5678"), "the panel sees the hint only");
        let (status, done) = status_of(&app, &started.id).await;
        assert_eq!((status, done), (StatusCode::OK, json!({"state": "done", "title": "OpenRouter"})));

        // The same return again (a reload of the page): nothing is exchanged twice.
        let (status, page) = request(
            &app,
            "GET",
            &format!("/api/v1/oauth/callback?code=good-code&state={}", started.id),
            [127, 0, 0, 1],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.contains("Вход уже выполнен."), "{page}");
        assert_eq!(service.exchanges.lock().unwrap().len(), 1);
    }

    async fn status_of(app: &Arc<App>, id: &str) -> (StatusCode, Value) {
        match status(State(app.clone()), admin(app), Path(id.to_string())).await {
            Ok(answer) => (answer.0, serde_json::to_value(&answer.1).unwrap()),
            Err(error) => (error.status, json!({"message": error.message})),
        }
    }

    #[tokio::test]
    async fn from_another_device_the_code_of_the_page_or_the_address_of_the_return_finishes_it() {
        let (service, sign_in) = service().await;
        let app = app();
        let target = provider(&app);

        let started = app.oauth.start(&sign_in, target.clone(), false).unwrap();
        shown(&service, &started.url);
        let done = complete(State(app.clone()), admin(&app), body(json!({"id": started.id, "text": " good-code \n"})))
            .await
            .unwrap();
        assert_eq!(done.1, SignInDone { title: "OpenRouter".into() });
        assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));

        app.brain.update("openrouter", &ProviderInput::from_value(json!({"key": ""})), None).unwrap();
        let again = app.oauth.start(&sign_in, target, true).unwrap();
        shown(&service, &again.url);
        let address = format!("http://localhost:7780/api/v1/oauth/callback?code=good-code&state={}", again.id);
        let done = complete(State(app.clone()), admin(&app), body(json!({"text": address}))).await.unwrap();
        assert_eq!(done.1.title, "OpenRouter");
        assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));
    }

    #[tokio::test]
    async fn a_forged_refused_or_cancelled_return_brings_no_key_and_the_panel_learns_why() {
        let (service, sign_in) = service().await;
        let app = app();
        let target = provider(&app);
        let started = app.oauth.start(&sign_in, target.clone(), true).unwrap();
        shown(&service, &started.url);

        let (status, page) =
            request(&app, "GET", "/api/v1/oauth/callback?code=good-code&state=forged", [127, 0, 0, 1]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.contains("устарел или начат не здесь"), "{page}");

        let refused = complete(State(app.clone()), admin(&app), body(json!({"id": started.id, "text": "stolen-code"})))
            .await
            .err()
            .unwrap();
        assert!(refused.message.contains("не подтвердил вход"), "{}", refused.message);
        assert!(!app.secrets.has("llm:openrouter:key"));
        assert_eq!(status_of(&app, &started.id).await.1["state"], json!("failed"));

        let declined = app.oauth.start(&sign_in, target, true).unwrap();
        let (status, page) = request(
            &app,
            "GET",
            &format!("/api/v1/oauth/callback?error=access_denied&state={}", declined.id),
            [127, 0, 0, 1],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.contains("Вход отменён"), "{page}");
        assert_eq!(
            status_of(&app, &declined.id).await.1,
            json!({"state": "failed", "message": "Вход отменён на странице сервиса. Нажмите «Войти» ещё раз."})
        );
        let empty = complete(State(app.clone()), admin(&app), body(json!({"id": declined.id, "text": "  "}))).await;
        assert!(empty.err().unwrap().message.starts_with("Вставьте код"));
        assert_eq!(status_of(&app, "nobody").await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_key_goes_only_to_the_provider_that_began_the_sign_in() {
        let (service, sign_in) = service().await;
        let app = app();
        let target = provider(&app);
        let started = app.oauth.start(&sign_in, target, false).unwrap();
        shown(&service, &started.url);
        app.brain.remove("openrouter", None).unwrap();
        // Another provider takes the id meanwhile: it is not the one who signed in.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        app.brain.add(&ProviderInput::from_value(json!({"preset": "openrouter"})), None).unwrap();
        let gone = complete(State(app.clone()), admin(&app), body(json!({"id": started.id, "text": "good-code"})))
            .await
            .err()
            .unwrap();
        assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.message);
        assert!(!app.secrets.has("llm:openrouter:key"));
        assert_eq!(status_of(&app, &started.id).await.1["state"], json!("failed"));
    }

    #[tokio::test]
    async fn chatgpts_return_on_auth_callback_keeps_the_tokens_here_and_the_card_shows_whose() {
        let (issuer, base) = tokens::tests::issuer().await;
        let app = app();
        let added = app.brain.add(&ProviderInput::from_value(json!({"preset": "chatgpt"})), None).unwrap();
        let target = app.brain.sign_in_target(&added.id).unwrap();
        let sign_in = SignIn { with: "ChatGPT", label: None, flow: Flow::Tokens(tokens::tests::flow(&base)) };
        let started = app.oauth.start(&sign_in, target, true).unwrap();
        *issuer.nonce.lock().unwrap() = param(&started.url, "nonce");

        let uri = format!("/auth/callback?code=good-code&state={}&client_id=oaiapp_9&scope=openid", started.id);
        let (status, page) = request(&app, "GET", &uri, [127, 0, 0, 1]).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.contains("Вход в «ChatGPT» выполнен"), "{page}");
        let record: TokenRecord = serde_json::from_str(&app.secrets.get("oauth:llm:chatgpt").unwrap()).unwrap();
        assert_eq!((record.client_id.as_str(), record.access_token.as_str()), ("oaiapp_9", "access-code"));
        assert!(!app.secrets.has("llm:chatgpt:key"));
        let view = serde_json::to_value(app.brain.view()).unwrap();
        let card = view["providers"].as_array().unwrap().iter().find(|card| card["id"] == "chatgpt").unwrap().clone();
        assert_eq!(card["account"], json!({"signedIn": true, "who": "o…@example.com"}));
        let seen = view.to_string();
        assert!(!seen.contains("access-code") && !seen.contains("refresh-code") && !seen.contains("oaiapp_9"));
    }

    #[tokio::test]
    async fn the_return_is_taken_from_home_only_and_the_rest_from_people_with_the_right() {
        let app = app();
        let (status, text) = request(&app, "GET", "/api/v1/oauth/callback?state=x&code=y", [8, 8, 8, 8]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(text.contains("Только из домашней сети."), "{text}");
        for (method, uri) in [("POST", "/api/v1/oauth/complete"), ("GET", "/api/v1/oauth/abc")] {
            let (status, text) = request(&app, method, uri, [127, 0, 0, 1]).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}: {text}");
        }
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
        let refused = complete(State(app.clone()), key, body(json!({"id": "x", "text": "y"}))).await.err().unwrap();
        assert_eq!(
            (refused.status, refused.message.as_str()),
            (StatusCode::FORBIDDEN, "Это для людей, не для ключей.")
        );
    }
}
