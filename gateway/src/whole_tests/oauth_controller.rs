//! The tests of `oauth/controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path};
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::{api_key_principal, user_principal};
use crate::core::identity::Role;
use crate::dialog::brain::brain_service::ProviderInput;
use crate::oauth::controller::*;
use crate::oauth::tests::{duowave_answer, param, service, shown, SERVICE_KEY};
use crate::oauth::{tokens, Flow, SignIn, Target, TokenRecord};
use crate::platform::http::{Body, Caller, Uses};

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
    let mut request = axum::http::Request::builder().method(method).uri(uri).body(axum::body::Body::empty()).unwrap();
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
    let started = app.oauth.start(&sign_in, target, true, &Default::default()).unwrap();
    shown(&service, &started.url);

    let (status, page) =
        request(&app, "GET", &format!("/api/v1/oauth/callback?code=good-code&state={}", started.id), [127, 0, 0, 1])
            .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.starts_with("<!doctype html>") && page.contains("Вход в «OpenRouter» выполнен"), "{page}");
    assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));
    let view = serde_json::to_string(&app.brain.view()).unwrap();
    assert!(!view.contains(SERVICE_KEY) && view.contains("…5678"), "the panel sees the hint only");
    let (status, done) = status_of(&app, &started.id).await;
    assert_eq!((status, done), (StatusCode::OK, json!({"state": "done", "title": "OpenRouter"})));

    // The same return again (a reload of the page): nothing is exchanged twice.
    let (status, page) =
        request(&app, "GET", &format!("/api/v1/oauth/callback?code=good-code&state={}", started.id), [127, 0, 0, 1])
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("Вход уже выполнен."), "{page}");
    assert_eq!(service.exchanges.lock().unwrap().len(), 1);
}

async fn status_of(app: &Arc<App>, id: &str) -> (StatusCode, Value) {
    match status(Uses::of(&app), admin(app), Path(id.to_string())).await {
        Ok(answer) => (answer.0, serde_json::to_value(&answer.1).unwrap()),
        Err(error) => (error.status, json!({"message": error.message})),
    }
}

#[tokio::test]
async fn from_another_device_the_code_of_the_page_or_the_address_of_the_return_finishes_it() {
    let (service, sign_in) = service().await;
    let app = app();
    let target = provider(&app);

    let started = app.oauth.start(&sign_in, target.clone(), false, &Default::default()).unwrap();
    shown(&service, &started.url);
    let done =
        complete(Uses::of(&app), Uses::of(&app), admin(&app), body(json!({"id": started.id, "text": " good-code \n"})))
            .await
            .unwrap();
    assert_eq!(done.1, SignInDone { title: "OpenRouter".into(), note: None });
    assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));

    app.brain.update("openrouter", &ProviderInput::from_value(json!({"key": ""})), None).unwrap();
    let again = app.oauth.start(&sign_in, target, true, &Default::default()).unwrap();
    shown(&service, &again.url);
    let address = format!("http://localhost:7780/api/v1/oauth/callback?code=good-code&state={}", again.id);
    let done = complete(Uses::of(&app), Uses::of(&app), admin(&app), body(json!({"text": address}))).await.unwrap();
    assert_eq!(done.1.title, "OpenRouter");
    assert_eq!(app.secrets.get("llm:openrouter:key").as_deref(), Some(SERVICE_KEY));
}

#[tokio::test]
async fn a_forged_refused_or_cancelled_return_brings_no_key_and_the_panel_learns_why() {
    let (service, sign_in) = service().await;
    let app = app();
    let target = provider(&app);
    let started = app.oauth.start(&sign_in, target.clone(), true, &Default::default()).unwrap();
    shown(&service, &started.url);

    let (status, page) =
        request(&app, "GET", "/api/v1/oauth/callback?code=good-code&state=forged", [127, 0, 0, 1]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("устарел или начат не здесь"), "{page}");

    let refused =
        complete(Uses::of(&app), Uses::of(&app), admin(&app), body(json!({"id": started.id, "text": "stolen-code"})))
            .await
            .err()
            .unwrap();
    assert!(refused.message.contains("не подтвердил вход"), "{}", refused.message);
    assert!(!app.secrets.has("llm:openrouter:key"));
    assert_eq!(status_of(&app, &started.id).await.1["state"], json!("failed"));

    let declined = app.oauth.start(&sign_in, target, true, &Default::default()).unwrap();
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
    let empty =
        complete(Uses::of(&app), Uses::of(&app), admin(&app), body(json!({"id": declined.id, "text": "  "}))).await;
    assert!(empty.err().unwrap().message.starts_with("Вставьте код"));
    assert_eq!(status_of(&app, "nobody").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_key_goes_only_to_the_provider_that_began_the_sign_in() {
    let (service, sign_in) = service().await;
    let app = app();
    let target = provider(&app);
    let started = app.oauth.start(&sign_in, target, false, &Default::default()).unwrap();
    shown(&service, &started.url);
    app.brain.remove("openrouter", None).unwrap();
    // Another provider takes the id meanwhile: it is not the one who signed in.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    app.brain.add(&ProviderInput::from_value(json!({"preset": "openrouter"})), None).unwrap();
    let gone =
        complete(Uses::of(&app), Uses::of(&app), admin(&app), body(json!({"id": started.id, "text": "good-code"})))
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
    let started = app.oauth.start(&sign_in, target, true, &Default::default()).unwrap();
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

/// DuoWave's sign-in against the fake service: its answer is `answer`; the return lands here.
async fn duowave_sign_in(app: &Arc<App>, answer: Value) -> (StatusCode, String) {
    let (service, sign_in) = service().await;
    *service.answer.lock().unwrap() = Some(answer);
    let target = app.brain.sign_in_target("duowave").unwrap();
    let started = app.oauth.start(&sign_in, target, true, &Default::default()).unwrap();
    shown(&service, &started.url);
    let uri = format!("/api/v1/oauth/callback?code=good-code&state={}", started.id);
    request(app, "GET", &uri, [127, 0, 0, 1]).await
}

fn choice(app: &Arc<App>) -> Value {
    let view = serde_json::to_value(app.brain.view()).unwrap();
    json!([view["choice"]["fast"], view["choice"]["smart"]])
}

fn card(app: &Arc<App>, id: &str) -> Value {
    let view = serde_json::to_value(app.brain.view()).unwrap();
    view["providers"].as_array().unwrap().iter().find(|card| card["id"] == id).unwrap().clone()
}

#[tokio::test]
async fn duowaves_sign_in_brings_the_key_the_models_the_profiles_and_the_access() {
    let app = app();
    app.brain.add(&ProviderInput::from_value(json!({"preset": "duowave"})), None).unwrap();
    app.brain.set_invite(Some("s_OfxFjzLq"), None).unwrap();
    assert!(app.brain.view().invite);

    let (status, page) = duowave_sign_in(&app, duowave_answer()).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Вход в «DuoWave» выполнен"), "{page}");
    assert_eq!(app.secrets.get("llm:duowave:key").as_deref(), Some("dw-test-key-1234"));
    let shown_card = card(&app, "duowave");
    assert_eq!(shown_card["models"], json!(["duowave-smart", "duowave-fast"]));
    assert_eq!(
        shown_card["access"],
        json!({"active": true, "tier": "trial", "plan": "Пробная неделя", "endsAt": "2026-11-27T01:15:00.000Z"})
    );
    // The fake exchange is not in api.duowave.dev's domain: its address is not taken, the preset's stays.
    assert_eq!(shown_card["baseUrl"], json!("https://api.duowave.dev/v1"));
    assert_eq!(
        choice(&app),
        json!([
            {"provider": "duowave", "model": "duowave-fast"},
            {"provider": "duowave", "model": "duowave-smart"}
        ])
    );
    assert!(!app.brain.view().invite, "the invitation went with a sign-in that brought a key");
    assert!(!serde_json::to_string(&app.brain.view()).unwrap().contains("dw-test-key-1234"));

    // The panel saves the card: the service's word on the access stays.
    app.brain.update("duowave", &ProviderInput::from_value(json!({"title": "Наш", "options": {}})), None).unwrap();
    assert_eq!(card(&app, "duowave")["access"]["active"], json!(true));
}

#[tokio::test]
async fn a_later_sign_in_changes_only_the_profiles_already_on_duowave_and_null_changes_none() {
    let app = app();
    app.brain.add(&ProviderInput::from_value(json!({"preset": "duowave"})), None).unwrap();
    app.brain.add(&ProviderInput::from_value(json!({"preset": "openrouter", "key": "sk-or-1"})), None).unwrap();
    duowave_sign_in(&app, duowave_answer()).await;
    // The person takes «сильный» elsewhere; the key is revoked, they connect again.
    let smart = json!({"provider": "openrouter", "model": "anthropic/claude-sonnet"});
    app.brain.choose(&serde_json::Map::from_iter([("smart".to_string(), smart.clone())]), None).unwrap();
    let mut again = duowave_answer();
    again["defaults"] = json!({"fast_model": "duowave-turbo", "smart_model": "duowave-smart"});
    duowave_sign_in(&app, again).await;
    assert_eq!(choice(&app), json!([{"provider": "duowave", "model": "duowave-turbo"}, smart]));

    // Null in the defaults: nothing changes.
    let mut quiet = duowave_answer();
    quiet["defaults"] = json!({"fast_model": null, "smart_model": null});
    duowave_sign_in(&app, quiet).await;
    assert_eq!(choice(&app), json!([{"provider": "duowave", "model": "duowave-turbo"}, smart]));
}

#[tokio::test]
async fn without_access_the_key_is_kept_the_profiles_stay_and_the_owner_is_told() {
    let app = app();
    app.brain.add(&ProviderInput::from_value(json!({"preset": "duowave"})), None).unwrap();
    let mut closed = duowave_answer();
    closed["models"] = json!([]);
    closed["access"] = json!({"active": false, "tier": null, "plan_title": null, "ends_at": null});
    let (status, page) = duowave_sign_in(&app, closed).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("доступа к моделям пока нет"), "{page}");
    assert_eq!(app.secrets.get("llm:duowave:key").as_deref(), Some("dw-test-key-1234"));
    assert_eq!(choice(&app), json!([null, null]));
    assert_eq!(card(&app, "duowave")["access"], json!({"active": false}));
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
    let refused =
        complete(Uses::of(&app), Uses::of(&app), key, body(json!({"id": "x", "text": "y"}))).await.err().unwrap();
    assert_eq!((refused.status, refused.message.as_str()), (StatusCode::FORBIDDEN, "Это для людей, не для ключей."));
}
