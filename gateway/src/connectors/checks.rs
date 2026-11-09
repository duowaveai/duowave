//! «Проверить» of the connectors the panel sets up: one request to each with its saved values, the answer in words —
//! never a secret, never the URL a token rides in. And the look at a feed before it is added.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::host_of;
use super::settings::{CheckItem, CheckResult, ConnectorId, ConnectorSettings};
use crate::config::WeatherConfig;
use crate::platform::http::ApiError;
use crate::skills::lookups::http::{Http, HttpGet};
use crate::skills::lookups::news_skill::{feed_name, feed_titles};

/// How long one check waits for its answer.
const WAIT: Duration = Duration::from_secs(8);

/// A feed as it answers now: its name and its first headlines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedPreview {
    pub url: String,
    pub title: Option<String>,
    pub headlines: Vec<String>,
    pub count: usize,
}

pub struct Checker {
    client: reqwest::Client,
    /// The Bot API: Telegram's, a test's fake.
    telegram_api: String,
    weather: WeatherConfig,
    get: Arc<dyn HttpGet>,
}

/// A request's failure in words, without its URL (a bot's token rides in it).
fn failure(error: reqwest::Error) -> String {
    if error.is_timeout() {
        format!("не ответил за {} с", WAIT.as_secs())
    } else if error.is_connect() {
        "не отвечает".into()
    } else {
        format!("{}", error.without_url())
    }
}

impl Checker {
    pub fn new(weather: WeatherConfig) -> Checker {
        #[cfg(feature = "pc")]
        let telegram_api = crate::calls::telegram::TELEGRAM_API.to_string();
        #[cfg(not(feature = "pc"))]
        let telegram_api = String::new();
        Checker { client: crate::utils::http::client(), telegram_api, weather, get: Arc::new(Http) }
    }

    /// Against fakes: tests.
    pub fn with(weather: WeatherConfig, telegram_api: &str, get: Arc<dyn HttpGet>) -> Checker {
        Checker { client: crate::utils::http::client(), telegram_api: telegram_api.to_string(), weather, get }
    }

    /// The connector's check with its saved values; kept as its state until it changes.
    pub async fn check(&self, settings: &ConnectorSettings, id: ConnectorId) -> CheckResult {
        let result = match id {
            _ if !id.available() => {
                if cfg!(feature = "pc") {
                    CheckResult::new(false, "пока недоступно — будет с обновлением")
                } else {
                    CheckResult::new(false, "в сборке для колонки звонков нет")
                }
            }
            ConnectorId::HomeAssistant => match settings.home_assistant() {
                Some(home) => self.home_assistant(&home.url, &home.token).await,
                None => CheckResult::new(false, "не настроено или выключено"),
            },
            ConnectorId::Telegram => match settings.telegram() {
                Some(telegram) => self.telegram(&telegram.token).await,
                None => CheckResult::new(false, "не настроено или выключено"),
            },
            ConnectorId::LiveKit => self.livekit(settings).await,
            ConnectorId::Sip => match settings.sip() {
                Some(sip) => self.sip(&sip.url, &sip.user, &sip.password).await,
                None => CheckResult::new(false, "не настроено или выключено"),
            },
            ConnectorId::Weather => self.weather(settings.weather_place().as_deref()).await,
            ConnectorId::News => self.news(settings).await,
        };
        settings.record_check(id, result.clone());
        result
    }

    async fn home_assistant(&self, url: &str, token: &str) -> CheckResult {
        let request = self.client.get(format!("{url}/api/config")).bearer_auth(token).timeout(WAIT);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return CheckResult::new(false, format!("{}: {}", host_of(url), failure(error))),
        };
        match response.status().as_u16() {
            200 => {
                let config: Value = response.json().await.unwrap_or_default();
                let version = config.get("version").and_then(Value::as_str).unwrap_or("?");
                let place = config.get("location_name").and_then(Value::as_str).filter(|place| !place.is_empty());
                CheckResult::new(
                    true,
                    match place {
                        Some(place) => format!("Home Assistant {version} · {place}"),
                        None => format!("Home Assistant {version}"),
                    },
                )
            }
            401 | 403 => CheckResult::new(false, "токен не подходит"),
            404 => CheckResult::new(false, "по этому адресу не Home Assistant"),
            status => CheckResult::new(false, format!("ответил HTTP {status}")),
        }
    }

    async fn telegram(&self, token: &str) -> CheckResult {
        let request = self.client.get(format!("{}/bot{token}/getMe", self.telegram_api)).timeout(WAIT);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return CheckResult::new(false, format!("Telegram {}", failure(error))),
        };
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or_default();
        match (status, body.pointer("/result/username").and_then(Value::as_str)) {
            (200, Some(name)) => CheckResult::new(true, format!("бот @{name}")),
            (401 | 404, _) => CheckResult::new(false, "токен не подходит"),
            (status, _) => CheckResult::new(false, format!("Telegram ответил HTTP {status}")),
        }
    }

    #[cfg(feature = "pc")]
    async fn livekit(&self, settings: &ConnectorSettings) -> CheckResult {
        let Some(livekit) = settings.livekit() else {
            return CheckResult::new(false, "не настроено или выключено");
        };
        // The server's API is HTTP on the same port as its socket.
        let base = match livekit.url.split_once("://") {
            Some(("wss", rest)) => format!("https://{rest}"),
            Some(("ws", rest)) => format!("http://{rest}"),
            _ => livekit.url.clone(),
        };
        let token = crate::calls::livekit::token::list_token(&livekit);
        let request = self
            .client
            .post(format!("{}/twirp/livekit.RoomService/ListRooms", base.trim_end_matches('/')))
            .bearer_auth(token)
            .json(&serde_json::json!({}))
            .timeout(WAIT);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return CheckResult::new(false, format!("{}: {}", host_of(&livekit.url), failure(error))),
        };
        match response.status().as_u16() {
            200 => {
                let rooms: Value = response.json().await.unwrap_or_default();
                let count = rooms.get("rooms").and_then(Value::as_array).map_or(0, Vec::len);
                CheckResult::new(true, format!("сервер отвечает · комнат сейчас: {count}"))
            }
            401 | 403 => CheckResult::new(false, "ключ или секрет не подходят"),
            status => CheckResult::new(false, format!("ответил HTTP {status}")),
        }
    }

    #[cfg(not(feature = "pc"))]
    async fn livekit(&self, _settings: &ConnectorSettings) -> CheckResult {
        CheckResult::new(false, "в сборке для колонки звонков нет")
    }

    async fn sip(&self, url: &str, user: &str, password: &str) -> CheckResult {
        let request = self
            .client
            .get(format!("{}/ari/asterisk/info", url.trim_end_matches('/')))
            .basic_auth(user, Some(password))
            .timeout(WAIT);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return CheckResult::new(false, format!("{}: {}", host_of(url), failure(error))),
        };
        match response.status().as_u16() {
            200 => {
                let info: Value = response.json().await.unwrap_or_default();
                match info.pointer("/system/version").and_then(Value::as_str) {
                    Some(version) => CheckResult::new(true, format!("Asterisk {version}")),
                    None => CheckResult::new(true, "Asterisk отвечает"),
                }
            }
            401 | 403 => CheckResult::new(false, "логин или пароль не подходят"),
            404 => CheckResult::new(false, "по этому адресу нет ARI"),
            status => CheckResult::new(false, format!("ответил HTTP {status}")),
        }
    }

    /// The place found and the temperature there now; without a place, whether the services answer.
    async fn weather(&self, place: Option<&str>) -> CheckResult {
        let cancel = CancellationToken::new();
        let asked = place.unwrap_or("Москва");
        let url = format!(
            "{}?name={}&count=1&language=ru&format=json",
            self.weather.geocode_url,
            crate::skills::js::encode_uri_component(asked)
        );
        let found = match crate::skills::lookups::http::get_json(self.get.as_ref(), &url, &cancel, Vec::new()).await {
            Ok(found) => found,
            Err(error) => return CheckResult::new(false, format!("поиск города не ответил: {error:#}")),
        };
        let Some(first) = found.get("results").and_then(Value::as_array).and_then(|results| results.first()) else {
            return CheckResult::new(false, format!("не нашла город «{asked}»"));
        };
        let name = first.get("name").and_then(Value::as_str).unwrap_or(asked);
        let country = first.get("country").and_then(Value::as_str);
        let (latitude, longitude) = (first.get("latitude"), first.get("longitude"));
        let url = format!(
            "{}?latitude={}&longitude={}&current=temperature_2m",
            self.weather.forecast_url,
            latitude.map(Value::to_string).unwrap_or_default(),
            longitude.map(Value::to_string).unwrap_or_default()
        );
        let forecast = match crate::skills::lookups::http::get_json(self.get.as_ref(), &url, &cancel, Vec::new()).await
        {
            Ok(forecast) => forecast,
            Err(error) => return CheckResult::new(false, format!("прогноз не ответил: {error:#}")),
        };
        let now = forecast.pointer("/current/temperature_2m").and_then(Value::as_f64);
        let named = match country {
            Some(country) => format!("{name}, {country}"),
            None => name.to_string(),
        };
        match (place, now) {
            (Some(_), Some(now)) => CheckResult::new(true, format!("{named} · сейчас {}°", signed(now))),
            (Some(_), None) => CheckResult::new(true, named),
            (None, _) => CheckResult::new(true, "прогноз отвечает; город — по часовому поясу колонки"),
        }
    }

    /// Every feed in the list, the ones switched off too: how many headlines each gives now.
    async fn news(&self, settings: &ConnectorSettings) -> CheckResult {
        let feeds = settings.all_news_feeds();
        if feeds.is_empty() {
            return CheckResult::new(false, "лент нет");
        }
        let looks = futures::future::join_all(feeds.iter().map(|feed| self.preview_of(&feed.url))).await;
        let items: Vec<CheckItem> = feeds
            .iter()
            .zip(looks)
            .map(|(feed, look)| match look {
                Ok(preview) if preview.count > 0 => {
                    CheckItem {
                        key: feed.url.clone(), ok: true, text: format!("заголовков: {}", preview.count)
                    }
                }
                Ok(_) => {
                    CheckItem { key: feed.url.clone(), ok: false, text: "не лента новостей".into() }
                }
                Err(problem) => CheckItem { key: feed.url.clone(), ok: false, text: problem },
            })
            .collect();
        let on: Vec<&CheckItem> =
            items.iter().zip(&feeds).filter(|(_, feed)| feed.enabled).map(|(item, _)| item).collect();
        let working = on.iter().filter(|item| item.ok).count();
        let words = match (on.len() % 10, on.len() % 100) {
            (_, 11..=14) => "лент",
            (1, _) => "лента",
            (2..=4, _) => "ленты",
            _ => "лент",
        };
        let mut result = CheckResult::new(
            working > 0,
            match (working, on.len()) {
                (_, 0) => "все ленты выключены".to_string(),
                (1, 1) => "лента отвечает".to_string(),
                (working, all) if working == all => format!("отвечают все {all} {words}"),
                (working, all) => format!("отвечают {working} из {all}"),
            },
        );
        result.items = items;
        result
    }

    /// A feed as it answers now, before it is added.
    pub async fn preview(&self, url: &str) -> Result<FeedPreview, ApiError> {
        let parsed = url::Url::parse(url.trim()).ok().filter(|parsed| matches!(parsed.scheme(), "http" | "https"));
        if parsed.is_none() {
            return Err(ApiError::bad_request("Нужен адрес ленты вида https://…"));
        }
        let preview = self.preview_of(url.trim()).await.map_err(ApiError::bad_request)?;
        if preview.count == 0 {
            return Err(ApiError::bad_request("По этому адресу нет ленты новостей: ни одного заголовка."));
        }
        Ok(preview)
    }

    async fn preview_of(&self, url: &str) -> Result<FeedPreview, String> {
        let cancel = CancellationToken::new();
        let answer = tokio::time::timeout(WAIT, self.get.get(url.to_string(), Vec::new(), cancel)).await;
        let text = match answer {
            Err(_) => return Err(format!("не ответила за {} с", WAIT.as_secs())),
            Ok(Err(_)) => return Err("не отвечает".into()),
            Ok(Ok(answer)) if !(200..300).contains(&answer.status) => {
                return Err(format!("ответила HTTP {}", answer.status))
            }
            Ok(Ok(answer)) => answer.text,
        };
        let titles = feed_titles(&text);
        Ok(FeedPreview {
            url: url.to_string(),
            title: feed_name(&text),
            count: titles.len(),
            headlines: titles.into_iter().take(5).collect(),
        })
    }
}

/// «+12», «−3», «0»: as the forecast says it.
fn signed(degrees: f64) -> String {
    let rounded = degrees.round() as i64;
    match rounded {
        0 => "0".into(),
        n if n > 0 => format!("+{n}"),
        n => format!("−{}", -n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::secrets::SecretsService;
    use crate::settings::SettingsService;
    use crate::storage::Db;
    use axum::extract::Query;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    fn connectors() -> ConnectorSettings {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        let seed = Config::for_tests(&Env::default()).connectors;
        ConnectorSettings::new(&seed, settings, secrets, events, std::path::PathBuf::from("messages"))
    }

    fn object(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        base
    }

    fn weather(base: &str) -> WeatherConfig {
        WeatherConfig { forecast_url: format!("{base}/forecast"), geocode_url: format!("{base}/search") }
    }

    /// Home Assistant, ARI, the Bot API, a geocoder, a forecast and two feeds, each with the one right key.
    async fn services() -> String {
        let router = Router::new()
            .route(
                "/api/config",
                get(|headers: HeaderMap| async move {
                    match headers.get("authorization").and_then(|value| value.to_str().ok()) {
                        Some("Bearer good-ha-token") => {
                            (StatusCode::OK, Json(json!({"version": "2026.10.1", "location_name": "Дом"})))
                        }
                        _ => (StatusCode::UNAUTHORIZED, Json(json!({}))),
                    }
                }),
            )
            .route(
                "/ari/asterisk/info",
                get(|headers: HeaderMap| async move {
                    // duowave:secret
                    match headers.get("authorization").and_then(|value| value.to_str().ok()) {
                        Some("Basic ZHVvd2F2ZTpzZWNyZXQ=") => {
                            (StatusCode::OK, Json(json!({"system": {"version": "20.5.0"}})))
                        }
                        _ => (StatusCode::UNAUTHORIZED, Json(json!({}))),
                    }
                }),
            )
            .route("/bot123:good/getMe", get(|| async { Json(json!({"ok": true, "result": {"username": "home_bot"}})) }))
            .route(
                "/bot123:bad/getMe",
                get(|| async { (StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "description": "Unauthorized"}))) }),
            )
            .route(
                "/search",
                get(|Query(query): Query<HashMap<String, String>>| async move {
                    if query.get("name").map(String::as_str) == Some("Атлантида") {
                        return Json(json!({}));
                    }
                    Json(json!({"results": [{"name": "Новосибирск", "country": "Россия", "latitude": 55.04, "longitude": 82.93}]}))
                }),
            )
            .route("/forecast", get(|| async { Json(json!({"current": {"temperature_2m": -3.4}})) }))
            .route(
                "/feed.xml",
                get(|| async {
                    "<rss><channel><title>Своя лента</title><item><title>Первая</title></item><item><title>Вторая</title></item></channel></rss>"
                }),
            )
            .route("/page.html", get(|| async { "<html><title>Не лента</title></html>" }))
            .route("/twirp/livekit.RoomService/ListRooms", post(|| async { Json(json!({"rooms": [{"name": "home"}]})) }));
        serve(router).await
    }

    #[tokio::test]
    async fn says_in_words_what_each_service_answers() {
        let base = services().await;
        let checker = Checker::with(weather(&base), &base, Arc::new(Http));
        let settings = connectors();
        let check = |id| checker.check(&settings, id);

        assert_eq!(check(ConnectorId::HomeAssistant).await.text, "не настроено или выключено");
        settings
            .update(ConnectorId::HomeAssistant, &object(json!({"url": base, "token": "good-ha-token"})), None)
            .unwrap();
        let home = check(ConnectorId::HomeAssistant).await;
        assert_eq!((home.ok, home.text.as_str()), (true, "Home Assistant 2026.10.1 · Дом"));
        assert_eq!(
            settings.status(ConnectorId::HomeAssistant),
            crate::core::connectors::ConnectorStatus::ready("Home Assistant 2026.10.1 · Дом")
        );
        settings.update(ConnectorId::HomeAssistant, &object(json!({"token": "wrong-ha-token"})), None).unwrap();
        assert_eq!(check(ConnectorId::HomeAssistant).await.text, "токен не подходит");

        settings.update(ConnectorId::Weather, &object(json!({"location": "Новосибирск"})), None).unwrap();
        assert_eq!(check(ConnectorId::Weather).await.text, "Новосибирск, Россия · сейчас −3°");
        settings.update(ConnectorId::Weather, &object(json!({"location": "Атлантида"})), None).unwrap();
        assert_eq!(check(ConnectorId::Weather).await.text, "не нашла город «Атлантида»");

        settings
            .update(
                ConnectorId::News,
                &object(json!({"feeds": [
                    {"url": format!("{base}/feed.xml")},
                    {"url": format!("{base}/page.html")},
                    {"url": format!("{base}/gone.xml"), "enabled": false},
                ]})),
                None,
            )
            .unwrap();
        let news = check(ConnectorId::News).await;
        assert_eq!((news.ok, news.text.as_str()), (true, "отвечают 1 из 2"));
        let texts: Vec<&str> = news.items.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, ["заголовков: 2", "не лента новостей", "ответила HTTP 404"]);
        let preview = checker.preview(&format!("{base}/feed.xml")).await.unwrap();
        assert_eq!((preview.title.as_deref(), preview.headlines.len()), (Some("Своя лента"), 2));
        assert_eq!(
            checker.preview(&format!("{base}/page.html")).await.unwrap_err().message,
            "По этому адресу нет ленты новостей: ни одного заголовка."
        );
        assert_eq!(
            checker.preview("file:///etc/passwd").await.unwrap_err().message,
            "Нужен адрес ленты вида https://…"
        );

        if cfg!(feature = "pc") {
            settings.update(ConnectorId::Telegram, &object(json!({"token": "123:good"})), None).unwrap();
            assert_eq!(check(ConnectorId::Telegram).await.text, "бот @home_bot");
            settings.update(ConnectorId::Telegram, &object(json!({"token": "123:bad"})), None).unwrap();
            let bad = check(ConnectorId::Telegram).await;
            assert_eq!((bad.ok, bad.text.as_str()), (false, "токен не подходит"));
            assert!(!bad.text.contains("123:bad"));

            if cfg!(feature = "livekit") {
                let ws = base.replacen("http://", "ws://", 1);
                settings
                    .update(
                        ConnectorId::LiveKit,
                        &object(
                            json!({"url": ws, "apiKey": "APIkey", "apiSecret": "a-secret-of-thirty-two-characters!!"}),
                        ),
                        None,
                    )
                    .unwrap();
                assert_eq!(check(ConnectorId::LiveKit).await.text, "сервер отвечает · комнат сейчас: 1");
            }
        }
    }

    /// LiveKit's wording when its SDK is not in this build: «скоро», not a check that cannot run.
    #[tokio::test]
    async fn livekit_and_sip_say_they_are_coming() {
        let settings = connectors();
        let checker = Checker::with(weather("https://weather.invalid"), "https://nothing.invalid", Arc::new(Http));
        for id in [ConnectorId::LiveKit, ConnectorId::Sip] {
            if id == ConnectorId::LiveKit && cfg!(feature = "livekit") {
                continue;
            }
            let outcome = checker.check(&settings, id).await;
            assert!(!outcome.ok, "{id:?}");
            assert!(outcome.text.contains("недоступно"), "the wording says it is coming: {}", outcome.text);
        }
    }

    #[tokio::test]
    async fn a_service_away_is_said_without_its_address_secrets() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let checker = Checker::with(weather(&base), &base, Arc::new(Http));
        let settings = connectors();
        settings
            .update(ConnectorId::HomeAssistant, &object(json!({"url": base, "token": "ha-token-123456"})), None)
            .unwrap();
        let away = checker.check(&settings, ConnectorId::HomeAssistant).await;
        assert!(!away.ok);
        assert!(away.text.ends_with("не отвечает"), "{}", away.text);
        assert!(!away.text.contains("ha-token"));
    }

    #[test]
    fn says_degrees_with_their_sign() {
        assert_eq!(
            (signed(12.4), signed(-3.4), signed(0.2), signed(-0.4)),
            ("+12".into(), "−3".into(), "0".into(), "0".into())
        );
    }
}
