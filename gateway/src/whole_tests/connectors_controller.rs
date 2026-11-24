//! The tests of `connectors/controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body as HttpBody;
use axum::extract::{ConnectInfo, Path};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::app;
use crate::app::App;
use crate::config::{Config, Env};
use crate::connectors::controller::*;
use crate::core::identity::principal::{api_key_principal, user_principal};
use crate::core::identity::Role;
use crate::platform::http::{ApiResult, Body, Caller, Uses};

fn app_with(env: &[(&str, &str)]) -> Arc<App> {
    let mut pairs = vec![("STATE_DIR", ""), ("DIALOG_PROVIDER", "none")];
    pairs.extend_from_slice(env);
    App::build(Config::for_tests(&Env::from_pairs(pairs))).unwrap()
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
async fn shows_what_env_seeded_takes_a_change_and_never_gives_a_secret_back() {
    let app = app_with(&[
        ("TELEGRAM_BOT_TOKEN", "123456:seeded-token-abcdef"),
        ("TELEGRAM_CONTACTS", "мама=111222333"),
        ("NEWS_FEEDS", "https://tass.ru/rss/v2.xml"),
    ]);
    let state = || Uses::of(&app);
    let (status, telegram) = body(view(state(), admin(&app), Path("telegram".into())).await);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(telegram["fromEnv"], json!(true));
    assert_eq!(telegram["secrets"]["token"]["set"], json!(true));
    assert_eq!(telegram["busy"], json!(false));
    assert!(!telegram.to_string().contains("seeded-token") && !telegram.to_string().contains("111222333"));

    let change = json!({"url": "http://192.168.1.5:8123", "token": "ha-token-123456789"});
    let (status, home) = body(update(state(), admin(&app), Path("home-assistant".into()), Body(change)).await);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(home["status"]["state"], json!("configured"));
    assert!(!home.to_string().contains("ha-token"));
    let list = app.connectors.views().await;
    let listed = list.iter().find(|view| view.manifest.id == "home-assistant").unwrap();
    assert_eq!(listed.status.detail.as_deref(), Some("192.168.1.5:8123"), "the list sees it without a restart");

    let (status, cleared) = body(clear(state(), admin(&app), Path("home-assistant".into())).await);
    assert_eq!((status, cleared["status"]["state"].clone()), (StatusCode::OK, json!("unset")));
    let (status, _) = body(view(state(), admin(&app), Path("fax".into())).await);
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lets_the_household_see_the_list_and_the_catalog_but_not_change_or_read_settings() {
    let app = app_with(&[]);
    let state = || Uses::of(&app);
    let (status, catalog) = body(catalog(member(&app)).await);
    assert_eq!(status, StatusCode::OK);
    assert!(catalog["feeds"].as_array().unwrap().len() >= 25);
    let (status, _) = body(view(state(), member(&app), Path("telegram".into())).await);
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) =
        body(update(state(), member(&app), Path("weather".into()), Body(json!({"location": "Омск"}))).await);
    assert_eq!(status, StatusCode::FORBIDDEN);
    let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".to_string()])));
    let (status, answer) = body(view(state(), key, Path("weather".into())).await);
    assert_eq!((status, answer["message"].as_str()), (StatusCode::FORBIDDEN, Some("Это для людей, не для ключей.")));
}

#[tokio::test]
async fn the_routes_are_there_and_closed_without_signing_in() {
    let app = app_with(&[]);
    for (method, path) in [
        ("GET", "/api/v1/connectors/telegram/settings"),
        ("PUT", "/api/v1/connectors/telegram/settings"),
        ("DELETE", "/api/v1/connectors/sip/settings"),
        ("POST", "/api/v1/connectors/news/check"),
        ("GET", "/api/v1/connectors/telegram/chats"),
        ("POST", "/api/v1/connectors/news/preview"),
        ("GET", "/api/v1/connectors/news/catalog"),
    ] {
        let mut request = Request::builder().method(method).uri(path).body(HttpBody::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 20], 50_000))));
        let response = app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}: {text}");
    }
}
