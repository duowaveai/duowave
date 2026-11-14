//! The panel's routes for the connectors it sets up («Дом» → «Коннекторы»). Their settings, a change, «Очистить»,
//! «Проверить», who wrote to the bot, a feed looked at before it is added: a person with the right of connectors,
//! from home — as the MCP servers (a token is someone's account). The news catalog: whoever sees the connectors.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::checks::{Checker, FeedPreview};
use super::settings::{ConnectorId, ConnectorSettingsView};
use crate::app::App;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::core::identity::PrincipalKind;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::lookups::news_catalog::{NewsCatalog, NEWS_CATALOG};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/connectors/news/catalog", get(catalog))
        .route("/api/v1/connectors/news/preview", post(preview))
        .route("/api/v1/connectors/telegram/chats", get(chats))
        .route("/api/v1/connectors/{id}/settings", get(view).put(update).delete(clear))
        .route("/api/v1/connectors/{id}/check", post(check))
}

/// From home, with the right of connectors, a person: never a script's key.
fn manager(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if caller.permitted(CONNECTORS_MANAGE)?.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    Ok(())
}

fn connector(id: &str) -> Result<ConnectorId, ApiError> {
    ConnectorId::parse(id).ok_or_else(|| ApiError::not_found(crate::tr!("Нет такого подключения.")))
}

/// A connector's settings, and whether a call goes on through it now (a change would end it).
#[derive(Debug, Serialize)]
struct Settings {
    #[serde(flatten)]
    view: ConnectorSettingsView,
    busy: bool,
}

fn answer(app: &App, id: ConnectorId) -> Settings {
    Settings { view: app.connector_settings.view(id), busy: busy(app, id) }
}

#[cfg(feature = "pc")]
fn busy(app: &App, id: ConnectorId) -> bool {
    match id {
        ConnectorId::Sip => app.calls.busy_through("sip:"),
        ConnectorId::LiveKit => app.calls.busy_through("livekit:"),
        _ => false,
    }
}

#[cfg(not(feature = "pc"))]
fn busy(_app: &App, _id: ConnectorId) -> bool {
    false
}

async fn view(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    Ok(ok(answer(&app, connector(&id)?)))
}

async fn update(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    let Value::Object(mut input) = body else {
        return Err(ApiError::bad_request(crate::tr!("Нужен объект с полями.")));
    };
    if id == ConnectorId::Telegram {
        strangers_to_chats(&app, &mut input)?;
    }
    app.connector_settings.update(id, &input, caller.actor())?;
    Ok(ok(answer(&app, id)))
}

/// A contact made of someone who wrote to the bot (`{name, stranger}`): their chat, which the panel never sees.
#[cfg(feature = "pc")]
fn strangers_to_chats(app: &App, input: &mut Map<String, Value>) -> Result<(), ApiError> {
    let Some(Value::Array(contacts)) = input.get_mut("contacts") else { return Ok(()) };
    for contact in contacts.iter_mut().filter_map(Value::as_object_mut) {
        let Some(stranger) = contact.remove("stranger") else { continue };
        let found = stranger.as_str().and_then(|handle| app.calls.telegram.stranger(handle));
        let Some(found) = found else {
            return Err(ApiError::bad_request(crate::tr!(
                "Этого чата уже нет среди написавших боту: откройте лист заново."
            )));
        };
        contact.insert("to".into(), Value::String(found.chat_id));
    }
    Ok(())
}

#[cfg(not(feature = "pc"))]
fn strangers_to_chats(_app: &App, _input: &mut Map<String, Value>) -> Result<(), ApiError> {
    Ok(())
}

async fn clear(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    app.connector_settings.clear(id, caller.actor())?;
    Ok(ok(answer(&app, id)))
}

async fn check(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    Checker::new(app.config.skills.weather.clone()).check(&app.connector_settings, id).await;
    Ok(ok(answer(&app, id)))
}

/// Who wrote to the bot lately and is no contact: a name and the chat's last digits.
async fn chats(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<Value>> {
    manager(&caller)?;
    #[cfg(feature = "pc")]
    let strangers: Vec<Value> =
        app.calls.telegram.strangers().iter().filter_map(|stranger| serde_json::to_value(stranger).ok()).collect();
    #[cfg(not(feature = "pc"))]
    let strangers: Vec<Value> = {
        let _ = &app;
        Vec::new()
    };
    Ok(ok(strangers))
}

async fn catalog(caller: Caller) -> ApiResult<NewsCatalog> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(NEWS_CATALOG.clone()))
}

#[derive(Debug, Default, Deserialize)]
struct PreviewBody {
    #[serde(default)]
    url: String,
}

async fn preview(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<PreviewBody>) -> ApiResult<FeedPreview> {
    manager(&caller)?;
    Ok(ok(Checker::new(app.config.skills.weather.clone()).preview(&body.url).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{api_key_principal, user_principal};
    use crate::core::identity::Role;
    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::net::SocketAddr;
    use tower::ServiceExt;

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
        let state = || State(app.clone());
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
        let state = || State(app.clone());
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
        assert_eq!(
            (status, answer["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Это для людей, не для ключей."))
        );
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
}
