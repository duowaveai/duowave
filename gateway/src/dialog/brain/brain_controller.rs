//! «Мозг» в пульте: провайдеры моделей (готовые и свои, со своими ключами) и выбор быстрой и умной модели. Видит
//! всякий, кто видит подключения; ключи не уходят ни в один ответ — только «задан · …a1b2». Добавляет и меняет
//! провайдеров человек с правом подключений, выбирает модели — с правом моделей. Действует со следующего вопроса,
//! без перезапуска. Провайдер, у сервиса которого есть вход через аккаунт (OpenRouter), получает ключ входом
//! (src/oauth): «Войти через …» начинается здесь, возврат принимает /api/v1/oauth.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::Router;
use serde_json::{json, Map, Value};

use super::brain_service::{BrainView, ProviderCheck, ProviderInput, ProviderView};
use crate::app::App;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW, MODELS_MANAGE};
use crate::core::identity::{Principal, PrincipalKind};
use crate::oauth::SignInStart;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/brain", get(view))
        .route("/api/v1/brain/providers", post(add))
        .route("/api/v1/brain/providers/{id}", patch(update).delete(remove))
        .route("/api/v1/brain/providers/{id}/check", post(check))
        .route("/api/v1/brain/providers/{id}/models", post(models))
        .route("/api/v1/brain/providers/{id}/signin", post(sign_in))
        .route("/api/v1/brain/choice", put(choose))
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

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<BrainView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(app.brain.view()))
}

async fn add(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<ProviderView> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    Ok(created(app.brain.add(&ProviderInput::from_value(body), caller.actor())?))
}

async fn update(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<ProviderView> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    Ok(ok(app.brain.update(&id, &ProviderInput::from_value(body), caller.actor())?))
}

async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> Result<Response, ApiError> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    app.brain.remove(&id, caller.actor())?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn check(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<ProviderCheck> {
    caller.permitted(CONNECTORS_MANAGE)?;
    Ok(ok(app.brain.check(&id, body.get("model")).await?))
}

async fn models(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    match app.brain.models(&id).await {
        Ok(models) => Ok(ok(json!({"models": models}))),
        Err(error) => Err(match error.downcast::<ApiError>() {
            Ok(api) => api,
            // A plain failure is a 500, as Nest answered it; the log gets the reason without secrets.
            Err(error) => ApiError::internal(app.secrets.redact(&format!("models of {id}: {error:#}"))),
        }),
    }
}

/// «Войти через …»: the service's page for the owner. The panel open on the gateway's own computer gets the browser
/// back by itself; from another device the page shows a code to paste — a return to localhost would not reach here.
async fn sign_in(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<SignInStart> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    Ok(ok(app.brain.sign_in(&id, &app.oauth, caller.zone == Zone::Local)?))
}

async fn choose(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<BrainView> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(ok(app.brain.choose(&object(body), caller.actor())?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, user_principal};
    use crate::core::identity::Role;
    use crate::dialog::brain::brain_service::tests::service;
    use axum::body::Body as AxumBody;
    use axum::extract::ConnectInfo;
    use http_body_util::BodyExt;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    fn app() -> Arc<App> {
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", ""), ("DIALOG_PROVIDER", "none")]))).unwrap()
    }

    fn admin(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u-admin", "Админ", Role::Admin, None)))
    }

    fn member(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u-anya", "Аня", Role::Member, None)))
    }

    /// The status and the JSON the envelope would carry (the message alone for an error).
    fn body<T: serde::Serialize>(result: ApiResult<T>) -> (StatusCode, Value) {
        match result {
            Ok(api) => (api.0, serde_json::to_value(&api.1).unwrap()),
            Err(error) => (error.status, json!({"message": error.message})),
        }
    }

    #[tokio::test]
    async fn adds_a_provider_with_a_key_the_panel_never_shows_again_checks_it_and_answers_with_it_from_the_next_question(
    ) {
        let (_service, url) = service().await;
        let app = app();
        let (status, start) = body(view(State(app.clone()), admin(&app)).await);
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = start["presets"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(&ids[..4], &["claude", "openai", "chatgpt", "openrouter"]);
        assert_eq!(start["providers"], json!([]));
        assert_eq!(start["choice"], json!({"fast": null, "smart": null, "effort": "low"}));
        assert_eq!(start["status"]["source"], json!("env"));

        let added = json!({"preset": "custom-openai", "title": "Дома", "baseUrl": url, "key": "sk-test-key-1234"});
        let (status, added) = body(add(State(app.clone()), admin(&app), Body(added)).await);
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(added["id"], json!("custom-openai"));
        assert_eq!(added["title"], json!("Дома"));
        assert_eq!(added["key"], json!({"set": true, "hint": "…1234"}));
        assert_eq!(added["cloud"], json!(false));

        let (status, checked) =
            body(check(State(app.clone()), admin(&app), Path("custom-openai".into()), Body(json!({}))).await);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(checked["ok"], json!(true), "{checked}");
        assert_eq!(checked["model"], json!("big-model"), "no model given: the first listed");
        assert_eq!(checked["tools"], json!(true));

        let choice = json!({"fast": {"provider": "custom-openai", "model": "small-model"}, "smart": {"provider": "custom-openai", "model": "big-model"}});
        let (status, chosen) = body(choose(State(app.clone()), admin(&app), Body(choice)).await);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(chosen["status"]["source"], json!("panel"));
        assert_eq!(chosen["status"]["models"], json!({"fast": "small-model", "smart": "big-model"}));
        assert_eq!(chosen["status"]["cloud"], json!(false));
        assert_eq!(app.dialog.description(), "small-model, smart big-model via Дома (panel)");
        assert_eq!(app.dialog.brain().profiles.fast.unwrap().title, "Дома");

        let (_, seen) = body(view(State(app.clone()), admin(&app)).await);
        assert!(!seen.to_string().contains("sk-test-key"));
        assert!(!serde_json::to_string(&app.dialog.brain()).unwrap().contains("sk-test-key"));
    }

    #[tokio::test]
    async fn lets_the_household_see_the_brain_but_not_change_it() {
        let app = app();
        assert_eq!(body(view(State(app.clone()), member(&app)).await).0, StatusCode::OK);
        let added = add(State(app.clone()), member(&app), Body(json!({"preset": "ollama"}))).await;
        assert_eq!(body(added).0, StatusCode::FORBIDDEN);
        let chosen = choose(State(app.clone()), member(&app), Body(json!({"fast": null}))).await;
        assert_eq!(body(chosen).0, StatusCode::FORBIDDEN);
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
        let (status, refused) = body(add(State(app.clone()), key, Body(json!({"preset": "ollama"}))).await);
        assert_eq!(
            (status, refused["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Это для людей, не для ключей."))
        );
    }

    #[tokio::test]
    async fn keeps_the_provider_that_answers_then_lets_it_go_with_its_key_once_env_answers_again() {
        let (_service, url) = service().await;
        let app = app();
        let added = json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-home-secret-777"});
        body(add(State(app.clone()), admin(&app), Body(added)).await);
        let choice = json!({"fast": {"provider": "custom-openai", "model": "home-small"}});
        body(choose(State(app.clone()), admin(&app), Body(choice)).await);
        let busy = remove(State(app.clone()), admin(&app), Path("custom-openai".into())).await;
        assert_eq!(busy.unwrap_err().status, StatusCode::CONFLICT);
        let (_, back) = body(choose(State(app.clone()), admin(&app), Body(json!({"fast": null}))).await);
        assert_eq!(back["status"]["source"], json!("env"));
        assert_eq!(back["choice"], json!({"fast": null, "smart": null, "effort": "low"}));
        let gone = remove(State(app.clone()), admin(&app), Path("custom-openai".into())).await.unwrap();
        assert_eq!(gone.status(), StatusCode::NO_CONTENT);
        let (_, after) = body(view(State(app.clone()), admin(&app)).await);
        assert_eq!(after["providers"], json!([]));
        assert!(app.secrets.ids("llm:").is_empty());
        let failing = models(State(app.clone()), admin(&app), Path("custom-openai".into())).await;
        assert_eq!(failing.err().unwrap().status, StatusCode::NOT_FOUND);
    }

    fn param(url: &str, name: &str) -> Option<String> {
        url::Url::parse(url).unwrap().query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
    }

    #[tokio::test]
    async fn signs_in_to_openrouter_back_to_this_computer_or_with_a_code_from_another_device() {
        let app = app();
        body(add(State(app.clone()), admin(&app), Body(json!({"preset": "openrouter"}))).await);
        let (status, here) = body(sign_in(State(app.clone()), admin(&app), Path("openrouter".into())).await);
        assert_eq!(status, StatusCode::OK, "{here}");
        assert_eq!(here["mode"], json!("here"));
        let url = here["url"].as_str().unwrap();
        assert!(url.starts_with("https://openrouter.ai/auth?"), "{url}");
        let port = app.config.app.public_port.unwrap_or(app.config.app.port);
        assert_eq!(param(url, "callback_url"), Some(crate::oauth::return_url(port, "/api/v1/oauth/callback")));
        assert_eq!(param(url, "state").as_deref(), here["id"].as_str());
        assert_eq!(app.oauth.status(here["id"].as_str().unwrap()), Some(crate::oauth::SignInStatus::Waiting));

        // The panel on a phone: localhost there is not this computer, so the page shows a code.
        let mut phone = admin(&app);
        phone.zone = Zone::Home;
        let (_, code) = body(sign_in(State(app.clone()), phone, Path("openrouter".into())).await);
        assert_eq!(code["mode"], json!("code"));
        let url = code["url"].as_str().unwrap();
        assert_eq!((param(url, "callback_url"), param(url, "state")), (None, None));
        assert!(param(url, "code_challenge").is_some());
    }

    #[tokio::test]
    async fn a_provider_without_a_sign_in_and_people_without_the_right_get_none() {
        let (_service, url) = service().await;
        let app = app();
        let added = json!({"preset": "custom-openai", "baseUrl": url});
        body(add(State(app.clone()), admin(&app), Body(added)).await);
        let (status, refused) = body(sign_in(State(app.clone()), admin(&app), Path("custom-openai".into())).await);
        assert_eq!(
            (status, refused["message"].as_str()),
            (StatusCode::BAD_REQUEST, Some("Этот провайдер подключается только ключом."))
        );
        let (status, _) = body(sign_in(State(app.clone()), admin(&app), Path("nobody".into())).await);
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body(sign_in(State(app.clone()), member(&app), Path("custom-openai".into())).await).0,
            StatusCode::FORBIDDEN
        );
        let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
        assert_eq!(body(sign_in(State(app.clone()), key, Path("custom-openai".into())).await).0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_routes_are_there_and_closed_without_signing_in() {
        let app = app();
        for (method, uri) in [
            ("GET", "/api/v1/brain"),
            ("POST", "/api/v1/brain/providers"),
            ("PATCH", "/api/v1/brain/providers/x"),
            ("DELETE", "/api/v1/brain/providers/x"),
            ("POST", "/api/v1/brain/providers/x/check"),
            ("POST", "/api/v1/brain/providers/x/models"),
            ("POST", "/api/v1/brain/providers/x/signin"),
            ("PUT", "/api/v1/brain/choice"),
            ("GET", "/api/v1/persona"),
            ("PUT", "/api/v1/persona"),
            ("POST", "/api/v1/persona"),
        ] {
            let mut request = axum::http::Request::builder().method(method).uri(uri).body(AxumBody::empty()).unwrap();
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
            let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let answer: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(answer["message"], json!("Нужно войти."), "{method} {uri}");
        }
    }
}
