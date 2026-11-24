//! The tests of `dialog/brain/brain_controller.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::extract::{ConnectInfo, Path};
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::{api_key_principal, user_principal};
use crate::core::identity::Role;
use crate::dialog::brain::brain_controller::*;
use crate::dialog::brain::brain_service::tests::service;
use crate::platform::http::{ApiResult, Body, Caller, Uses, Zone};

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
async fn adds_a_provider_with_a_key_the_panel_never_shows_again_checks_it_and_answers_with_it_from_the_next_question() {
    let (_service, url) = service().await;
    let app = app();
    let (status, start) = body(view(Uses::of(&app), admin(&app)).await);
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = start["presets"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(&ids[..5], &["duowave", "claude", "openai", "chatgpt", "openrouter"]);
    assert_eq!(start["providers"], json!([]));
    assert_eq!(
        start["choice"],
        json!({"fast": null, "smart": null, "effort": "low", "autoBackup": true, "onLimit": "ask"})
    );
    assert_eq!(start["status"]["source"], json!("env"));

    let added = json!({"preset": "custom-openai", "title": "Дома", "baseUrl": url, "key": "sk-test-key-1234"});
    let (status, added) = body(add(Uses::of(&app), admin(&app), Body(added)).await);
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(added["id"], json!("custom-openai"));
    assert_eq!(added["title"], json!("Дома"));
    assert_eq!(added["key"], json!({"set": true, "hint": "…1234"}));
    assert_eq!(added["cloud"], json!(false));

    let (status, checked) =
        body(check(Uses::of(&app), admin(&app), Path("custom-openai".into()), Body(json!({}))).await);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(checked["ok"], json!(true), "{checked}");
    assert_eq!(checked["model"], json!("big-model"), "no model given: the first listed");
    assert_eq!(checked["tools"], json!(true));

    let choice = json!({"fast": {"provider": "custom-openai", "model": "small-model"}, "smart": {"provider": "custom-openai", "model": "big-model"}});
    let (status, chosen) = body(choose(Uses::of(&app), admin(&app), Body(choice)).await);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chosen["status"]["source"], json!("panel"));
    assert_eq!(chosen["status"]["models"], json!({"fast": "small-model", "smart": "big-model"}));
    assert_eq!(chosen["status"]["cloud"], json!(false));
    assert_eq!(app.dialog.description(), "small-model, smart big-model via Дома (panel)");
    assert_eq!(app.dialog.brain().profiles.fast.unwrap().title, "Дома");

    let (_, seen) = body(view(Uses::of(&app), admin(&app)).await);
    assert!(!seen.to_string().contains("sk-test-key"));
    assert!(!serde_json::to_string(&app.dialog.brain()).unwrap().contains("sk-test-key"));
}

#[tokio::test]
async fn lets_the_household_see_the_brain_but_not_change_it() {
    let app = app();
    assert_eq!(body(view(Uses::of(&app), member(&app)).await).0, StatusCode::OK);
    let added = add(Uses::of(&app), member(&app), Body(json!({"preset": "ollama"}))).await;
    assert_eq!(body(added).0, StatusCode::FORBIDDEN);
    let chosen = choose(Uses::of(&app), member(&app), Body(json!({"fast": null}))).await;
    assert_eq!(body(chosen).0, StatusCode::FORBIDDEN);
    let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
    let (status, refused) = body(add(Uses::of(&app), key, Body(json!({"preset": "ollama"}))).await);
    assert_eq!((status, refused["message"].as_str()), (StatusCode::FORBIDDEN, Some("Это для людей, не для ключей.")));
}

#[tokio::test]
async fn keeps_the_provider_that_answers_then_lets_it_go_with_its_key_once_env_answers_again() {
    let (_service, url) = service().await;
    let app = app();
    let added = json!({"preset": "custom-openai", "baseUrl": url, "key": "sk-home-secret-777"});
    body(add(Uses::of(&app), admin(&app), Body(added)).await);
    let choice = json!({"fast": {"provider": "custom-openai", "model": "home-small"}});
    body(choose(Uses::of(&app), admin(&app), Body(choice)).await);
    let busy = remove(Uses::of(&app), admin(&app), Path("custom-openai".into())).await;
    assert_eq!(busy.unwrap_err().status, StatusCode::CONFLICT);
    let (_, back) = body(choose(Uses::of(&app), admin(&app), Body(json!({"fast": null}))).await);
    assert_eq!(back["status"]["source"], json!("env"));
    assert_eq!(
        back["choice"],
        json!({"fast": null, "smart": null, "effort": "low", "autoBackup": true, "onLimit": "ask"})
    );
    let gone = remove(Uses::of(&app), admin(&app), Path("custom-openai".into())).await.unwrap();
    assert_eq!(gone.status(), StatusCode::NO_CONTENT);
    let (_, after) = body(view(Uses::of(&app), admin(&app)).await);
    assert_eq!(after["providers"], json!([]));
    assert!(app.secrets.ids("llm:").is_empty());
    let failing = models(Uses::of(&app), Uses::of(&app), admin(&app), Path("custom-openai".into())).await;
    assert_eq!(failing.err().unwrap().status, StatusCode::NOT_FOUND);
}

/// Rehearsal 16.11: the first steps look for Ollama and LM Studio; one that is not there answered 500 and an
/// ERROR in the log. Words now, and a 502.
#[tokio::test]
async fn a_local_model_server_that_is_not_there_is_said_in_words() {
    let app = app();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let ollama = json!({"preset": "ollama", "baseUrl": format!("http://127.0.0.1:{port}/v1")});
    assert_eq!(body(add(Uses::of(&app), admin(&app), Body(ollama)).await).0, StatusCode::CREATED);
    let (status, answer) = body(models(Uses::of(&app), Uses::of(&app), admin(&app), Path("ollama".into())).await);
    assert_eq!((status, answer["message"].as_str()), (StatusCode::BAD_GATEWAY, Some("Адрес не отвечает.")));
}

fn param(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url).unwrap().query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

#[tokio::test]
async fn signs_in_to_openrouter_back_to_this_computer_or_with_a_code_from_another_device() {
    let app = app();
    body(add(Uses::of(&app), admin(&app), Body(json!({"preset": "openrouter"}))).await);
    let (status, here) = body(
        sign_in(Uses::of(&app), Uses::of(&app), admin(&app), Path("openrouter".into()), Body(SignInBody::default()))
            .await,
    );
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
    let (_, code) = body(
        sign_in(Uses::of(&app), Uses::of(&app), phone, Path("openrouter".into()), Body(SignInBody::default())).await,
    );
    assert_eq!(code["mode"], json!("code"));
    let url = code["url"].as_str().unwrap();
    assert_eq!((param(url, "callback_url"), param(url, "state")), (None, None));
    assert!(param(url, "code_challenge").is_some());
}

#[tokio::test]
async fn duowaves_sign_in_carries_the_kept_invitation_and_the_device_name() {
    let app = app();
    body(add(Uses::of(&app), admin(&app), Body(json!({"preset": "duowave"}))).await);
    let start = |text: Value| {
        let app = app.clone();
        async move {
            sign_in(
                Uses::of(&app),
                Uses::of(&app),
                admin(&app),
                Path("duowave".into()),
                Body(serde_json::from_value(text).unwrap()),
            )
            .await
        }
    };
    let refused = start(json!({"invite": "s_<b>"})).await.err().unwrap();
    assert_eq!(refused.message, "Код приглашения не похож на настоящий. Проверьте его.");

    let started = start(json!({"invite": " s_OfxFjzLq ", "client": "Кабинет"})).await.unwrap().1;
    assert!(started.url.starts_with("https://my.duowave.app/connect?"), "{}", started.url);
    let param = |url: &str, name: &str| crate::oauth::tests::param(url, name);
    assert_eq!(param(&started.url, "start").as_deref(), Some("s_OfxFjzLq"));
    assert_eq!(param(&started.url, "client").as_deref(), Some("Кабинет"));
    assert!(app.brain.view().invite, "kept for the sign-ins that follow");

    // An empty body, as an older panel sends: the kept code still goes.
    let again = start(json!({})).await.unwrap().1;
    assert_eq!(param(&again.url, "start").as_deref(), Some("s_OfxFjzLq"));

    // Forgotten through its own route; OpenRouter never gets it.
    let view = invite(Uses::of(&app), admin(&app), Body(serde_json::from_value(json!({"code": null})).unwrap()))
        .await
        .unwrap();
    assert!(!view.1.invite);
    assert_eq!(param(&start(json!({})).await.unwrap().1.url, "start"), None);
    app.brain.set_invite(Some("s_OfxFjzLq"), None).unwrap();
    body(add(Uses::of(&app), admin(&app), Body(json!({"preset": "openrouter"}))).await);
    let theirs = sign_in(
        Uses::of(&app),
        Uses::of(&app),
        admin(&app),
        Path("openrouter".into()),
        Body(serde_json::from_value(json!({"invite": "s_OfxFjzLq"})).unwrap()),
    )
    .await
    .unwrap()
    .1;
    assert_eq!((param(&theirs.url, "start"), param(&theirs.url, "client")), (None, None));
}

#[tokio::test]
async fn a_provider_without_a_sign_in_and_people_without_the_right_get_none() {
    let (_service, url) = service().await;
    let app = app();
    let added = json!({"preset": "custom-openai", "baseUrl": url});
    body(add(Uses::of(&app), admin(&app), Body(added)).await);
    let (status, refused) = body(
        sign_in(Uses::of(&app), Uses::of(&app), admin(&app), Path("custom-openai".into()), Body(SignInBody::default()))
            .await,
    );
    assert_eq!(
        (status, refused["message"].as_str()),
        (StatusCode::BAD_REQUEST, Some("Этот провайдер подключается только ключом."))
    );
    let (status, _) = body(
        sign_in(Uses::of(&app), Uses::of(&app), admin(&app), Path("nobody".into()), Body(SignInBody::default())).await,
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body(
            sign_in(
                Uses::of(&app),
                Uses::of(&app),
                member(&app),
                Path("custom-openai".into()),
                Body(SignInBody::default())
            )
            .await
        )
        .0,
        StatusCode::FORBIDDEN
    );
    let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
    assert_eq!(
        body(
            sign_in(Uses::of(&app), Uses::of(&app), key, Path("custom-openai".into()), Body(SignInBody::default()))
                .await
        )
        .0,
        StatusCode::FORBIDDEN
    );
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
