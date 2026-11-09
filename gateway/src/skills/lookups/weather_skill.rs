//! The weather from an open API (Open-Meteo: no key): now, tomorrow or the week, for the place asked, the panel's
//! usual place («Дом» → «Коннекторы» → «Погода»), or the city of the station's time zone. The model puts the data into a short answer; the temperature
//! now goes to the display, as Yandex shows it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::http::{get_json, Http, HttpGet};
use crate::config::WeatherConfig;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::display::show_temperature;
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};
use crate::skills::text::normalize;

/// WMO weather codes, as Open-Meteo gives them.
const CODES: &[(&[i64], &str)] = &[
    (&[0], "ясно"),
    (&[1], "преимущественно ясно"),
    (&[2], "переменная облачность"),
    (&[3], "пасмурно"),
    (&[45, 48], "туман"),
    (&[51, 53, 55], "морось"),
    (&[56, 57], "ледяная морось"),
    (&[61], "небольшой дождь"),
    (&[63], "дождь"),
    (&[65], "сильный дождь"),
    (&[66, 67], "ледяной дождь"),
    (&[71], "небольшой снег"),
    (&[73], "снег"),
    (&[75], "сильный снег"),
    (&[77], "снежная крупа"),
    (&[80, 81], "ливень"),
    (&[82], "сильный ливень"),
    (&[85, 86], "снегопад"),
    (&[95], "гроза"),
    (&[96, 99], "гроза с градом"),
];

pub fn describe_code(code: f64) -> &'static str {
    CODES
        .iter()
        .find(|(codes, _)| codes.iter().any(|known| *known as f64 == code))
        .map(|(_, text)| *text)
        .unwrap_or("погода без описания")
}

/// `+12`, `-3`, `0`: a temperature as said.
fn signed(value: f64) -> String {
    format!("{}{}", if value > 0.0 { "+" } else { "" }, js::number_text(js::round(value)))
}

/// A number of the forecast: null reads as 0, a missing one as NaN, as the TS's arithmetic read them.
fn number(value: Option<&Value>) -> f64 {
    js::to_number(value)
}

/// A weather code as `includes` looks it up, without coercion: anything but a number (null where Open-Meteo has no
/// data, a string) is no code at all, not 0, «ясно».
fn code(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(f64::NAN)
}

#[derive(Debug, Clone, PartialEq)]
struct Place {
    name: String,
    latitude: f64,
    longitude: f64,
}

static PLACE_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)(?:в|во)\s+([\p{L}-]+(?:\s[\p{L}-]+)?)$").expect("a valid pattern"));
static COORDINATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(-?[0-9]+(?:\.[0-9]+)?)\s*,\s*(-?[0-9]+(?:\.[0-9]+)?)\s*$").expect("a valid pattern")
});

/// "в москве" at the end of a question: the place, as its word may stand there.
pub fn place_in(text: Option<&str>) -> Option<String> {
    let normalized = normalize(text.unwrap_or(""));
    PLACE_IN.captures(&normalized).and_then(|found| found.get(1)).map(|place| place.as_str().to_string())
}

/// The usual place now: the panel's («Дом» → «Коннекторы» → «Погода»), read on every question; None: the city of the
/// station's time zone.
pub type PlaceSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct WeatherSkill {
    config: WeatherConfig,
    place: PlaceSource,
    get: Arc<dyn HttpGet>,
    /// Places found by name: a city does not move, and its lookup is half the wait (~0,8 s of ~1,7 s).
    places: Mutex<HashMap<String, Place>>,
}

impl WeatherSkill {
    /// By the station's time zone until `with_place` gives a usual place.
    pub fn new(config: WeatherConfig, get: Option<Arc<dyn HttpGet>>) -> WeatherSkill {
        WeatherSkill {
            config,
            place: Arc::new(|| None),
            get: get.unwrap_or_else(|| Arc::new(Http)),
            places: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_place(mut self, place: PlaceSource) -> WeatherSkill {
        self.place = place;
        self
    }

    /// Looks the usual place up ahead (the panel's place or the city of the station's time zone), so the first
    /// question after a restart waits for the forecast alone. Live (02.11): the lookup and the forecast took 2 s
    /// and the temperature missed the response. The lookup's task, when one started.
    pub fn prepare(self: &Arc<Self>, timezone: Option<&str>) -> Option<tokio::task::JoinHandle<()>> {
        let place = self.usual_place(timezone).filter(|place| !place.is_empty())?;
        if self.places.lock().unwrap().contains_key(&place) {
            return None;
        }
        tokio::runtime::Handle::try_current().ok()?;
        let weather = self.clone();
        Some(tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let _ = tokio::time::timeout(Duration::from_secs(10), weather.locate(&place, &cancel)).await;
        }))
    }

    async fn forecast(&self, intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
        let param = |key: &str| intent.params.get(key).filter(|value| !value.is_null());
        let when = normalize(&param("when").or(param("day")).map(js::string_of).unwrap_or_default());
        let asked = match js::string_param(&intent.params, "place") {
            Some(place) => Some(place.to_string()),
            None => place_in(Some(&param("when").map(js::string_of).unwrap_or_default())),
        };
        let Some(place_name) =
            asked.or_else(|| self.usual_place(context.timezone.as_deref())).filter(|w| !w.is_empty())
        else {
            return Ok(Outcome::speech("Не знаю, для какого города узнать погоду."));
        };
        let Some(place) = self.locate(&place_name, &context.cancel).await? else {
            return Ok(Outcome::speech(format!("Не нашла город «{place_name}».")));
        };
        let url = format!(
            "{}?latitude={}&longitude={}&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m\
             &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
             &timezone=auto&forecast_days=7&wind_speed_unit=ms",
            self.config.forecast_url,
            js::number_text(place.latitude),
            js::number_text(place.longitude)
        );
        let forecast = get_json(self.get.as_ref(), &url, &context.cancel, Vec::new()).await?;
        let day = js::string_param(&intent.params, "day");
        let days = if when.contains("недел") || day == Some("week") {
            7
        } else if when.contains("завтра") || day == Some("tomorrow") {
            1
        } else {
            0
        };
        let text = describe(&forecast, &place.name, days);
        let current = forecast.get("current").filter(|current| js::truthy(Some(current)));
        let shown = current.filter(|_| days == 0).and_then(|now| show_temperature(number(now.get("temperature_2m"))));
        Ok(Outcome {
            data: Some(text.clone()),
            speech: Some(text),
            directives: shown.into_iter().collect(),
            ..Outcome::default()
        })
    }

    fn usual_place(&self, timezone: Option<&str>) -> Option<String> {
        if let Some(location) = (self.place)() {
            return Some(location);
        }
        let city = timezone?.split('/').next_back()?;
        Some(city.replace('_', " "))
    }

    /// "lat,lon" as is; a name through geocoding, with the case endings of "в Москве" tried off.
    async fn locate(&self, place: &str, cancel: &CancellationToken) -> anyhow::Result<Option<Place>> {
        if let Some(found) = COORDINATES.captures(place) {
            return Ok(Some(Place {
                name: "Здесь".into(),
                latitude: js::string_to_number(&found[1]),
                longitude: js::string_to_number(&found[2]),
            }));
        }
        if let Some(known) = self.places.lock().unwrap().get(place) {
            return Ok(Some(known.clone()));
        }
        let mut variants: Vec<String> = Vec::new();
        let ends = |end: char, with: &str| match place.strip_suffix(end) {
            Some(stem) => format!("{stem}{with}"),
            None => place.to_string(),
        };
        for variant in [place.to_string(), ends('е', "а"), ends('е', ""), ends('и', "ь")] {
            if !variants.contains(&variant) {
                variants.push(variant);
            }
        }
        for name in variants {
            let url = format!(
                "{}?name={}&count=1&language=ru&format=json",
                self.config.geocode_url,
                js::encode_uri_component(&name)
            );
            let found = get_json(self.get.as_ref(), &url, cancel, Vec::new()).await?;
            if let Some(first) = found.get("results").and_then(Value::as_array).and_then(|results| results.first()) {
                let located = Place {
                    name: first.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into()),
                    latitude: number(first.get("latitude")),
                    longitude: number(first.get("longitude")),
                };
                self.places.lock().unwrap().insert(place.to_string(), located.clone());
                return Ok(Some(located));
            }
        }
        Ok(None)
    }
}

fn describe(forecast: &Value, name: &str, days: usize) -> String {
    let daily = forecast.get("daily").filter(|daily| js::truthy(Some(daily)));
    let times: Vec<Value> =
        daily.and_then(|daily| daily.get("time")).and_then(Value::as_array).cloned().unwrap_or_default();
    let series = |key: &str, index: usize| daily.and_then(|daily| daily.get(key)).and_then(|values| values.get(index));
    let day = |index: usize| -> String {
        if daily.is_none() || index >= times.len() {
            return String::new();
        }
        let rain = series("precipitation_probability_max", index);
        let rain = if js::truthy(rain) {
            format!(", вероятность осадков {}%", rain.map(js::string_of).unwrap_or_default())
        } else {
            String::new()
        };
        format!(
            "от {} до {}, {}{rain}",
            signed(number(series("temperature_2m_min", index))),
            signed(number(series("temperature_2m_max", index))),
            describe_code(code(series("weather_code", index)))
        )
    };
    if days == 1 {
        return format!("{name}, завтра: {}.", day(1));
    }
    if days == 7 && daily.is_some() {
        let week: Vec<String> = times
            .iter()
            .take(7)
            .enumerate()
            .map(|(index, date)| format!("{}: {}", js::string_of(date), day(index)))
            .collect();
        return format!("{name}, неделя: {}.", week.join("; "));
    }
    let current = forecast.get("current").filter(|current| js::truthy(Some(current))).map(|now| {
        let wind = match now.get("wind_speed_10m") {
            Some(wind) => format!(", ветер {} м/с", js::number_text(js::round(number(Some(wind))))),
            None => String::new(),
        };
        format!(
            "сейчас {}, {}{wind}",
            signed(number(now.get("temperature_2m"))),
            describe_code(code(now.get("weather_code")))
        )
    });
    let parts: Vec<String> =
        [current.unwrap_or_default(), if daily.is_some() { format!("сегодня {}", day(0)) } else { String::new() }]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
    format!("{name}: {}.", parts.join("; "))
}

impl OwnSkill for WeatherSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Weather
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(self.forecast(intent, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::lookups::http::HttpText;
    use serde_json::json;

    /// Open-Meteo as far as the skill asks it: Москва by name, one forecast for everything.
    #[derive(Default)]
    struct OpenMeteo {
        asked: Mutex<Vec<String>>,
        /// Another forecast than the usual one.
        forecast: Option<Value>,
    }

    impl HttpGet for OpenMeteo {
        fn get(
            &self,
            url: String,
            _: Vec<(String, String)>,
            _: CancellationToken,
        ) -> BoxFuture<'static, anyhow::Result<HttpText>> {
            self.asked.lock().unwrap().push(url.clone());
            let text = if url.contains("geocoding") {
                let parsed = url::Url::parse(&url).unwrap();
                let name = parsed.query_pairs().find(|(key, _)| key == "name").map(|(_, value)| value.to_lowercase());
                if name.as_deref() == Some("москва") {
                    json!({"results": [{"name": "Москва", "latitude": 55.75, "longitude": 37.62}]})
                } else {
                    json!({})
                }
            } else {
                self.forecast.clone().unwrap_or_else(|| {
                    json!({
                        "current": {"temperature_2m": 12.4, "weather_code": 3, "wind_speed_10m": 3.2},
                        "daily": {
                            "time": ["2026-11-01", "2026-11-02"],
                            "weather_code": [3, 61],
                            "temperature_2m_max": [14, 9],
                            "temperature_2m_min": [6, 4],
                            "precipitation_probability_max": [10, 80],
                        },
                    })
                })
            };
            Box::pin(async move { Ok(HttpText { status: 200, text: text.to_string() }) })
        }
    }

    fn config() -> WeatherConfig {
        WeatherConfig {
            forecast_url: "https://api.example/forecast".into(),
            geocode_url: "https://geocoding.example/search".into(),
        }
    }

    fn context() -> ExecutionContext {
        let mut context = ExecutionContext::new("h:test", "2026-11-01T10:00:00Z".parse().unwrap());
        context.timezone = Some("Asia/Dubai".into());
        context
    }

    fn forecast(params: Value) -> Intent {
        Intent {
            capability: CapabilityName::Weather,
            action: "forecast".into(),
            params: params.as_object().cloned().unwrap(),
        }
    }

    #[tokio::test]
    async fn tells_the_weather_now_for_the_place_asked_its_case_ending_tried_off() {
        let skill = WeatherSkill::new(config(), Some(Arc::new(OpenMeteo::default())));
        assert_eq!(place_in(Some("завтра в москве")).as_deref(), Some("москве"));
        let outcome = skill.execute(&forecast(json!({"when": "в москве"})), &context()).await.unwrap();
        assert_eq!(
            outcome.data.as_deref(),
            Some("Москва: сейчас +12, пасмурно, ветер 3 м/с; сегодня от +6 до +14, пасмурно, вероятность осадков 10%.")
        );
        // The temperature now on the display, as Yandex shows it.
        assert_eq!(
            outcome.directives.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["draw_animation_directive"]
        );
    }

    #[tokio::test]
    async fn looks_a_place_up_once() {
        let get = Arc::new(OpenMeteo::default());
        let skill = WeatherSkill::new(config(), Some(get.clone()));
        skill.execute(&forecast(json!({"place": "Москва"})), &context()).await.unwrap();
        skill.execute(&forecast(json!({"place": "Москва"})), &context()).await.unwrap();
        let asked = get.asked.lock().unwrap();
        assert!(asked.iter().filter(|url| url.contains("geocoding")).count() <= 1);
        assert_eq!(asked.iter().filter(|url| url.contains("forecast")).count(), 2);
    }

    #[tokio::test]
    async fn looks_the_usual_place_up_when_the_station_connects_so_the_first_question_waits_for_the_forecast_alone() {
        let get = Arc::new(OpenMeteo::default());
        let usual =
            Arc::new(WeatherSkill::new(config(), Some(get.clone())).with_place(Arc::new(|| Some("Москва".into()))));
        usual.prepare(Some("Asia/Bangkok")).unwrap().await.unwrap();
        get.asked.lock().unwrap().clear();
        let outcome = usual.execute(&forecast(json!({})), &context()).await.unwrap();
        assert!(outcome.data.unwrap().starts_with("Москва: сейчас +12"));
        let hosts: Vec<String> = get
            .asked
            .lock()
            .unwrap()
            .iter()
            .map(|url| url::Url::parse(url).unwrap().host_str().unwrap().to_string())
            .collect();
        assert_eq!(hosts, ["api.example"]);
        assert!(usual.prepare(Some("Asia/Bangkok")).is_none(), "known already");
    }

    #[tokio::test]
    async fn tells_tomorrow() {
        let skill = WeatherSkill::new(config(), Some(Arc::new(OpenMeteo::default())));
        let outcome =
            skill.execute(&forecast(json!({"place": "Москва", "day": "tomorrow"})), &context()).await.unwrap();
        assert_eq!(
            outcome.data.as_deref(),
            Some("Москва, завтра: от +4 до +9, небольшой дождь, вероятность осадков 80%.")
        );
        assert!(outcome.directives.is_empty());
    }

    #[tokio::test]
    async fn tells_the_week_and_where_it_cannot_look() {
        let skill = WeatherSkill::new(config(), Some(Arc::new(OpenMeteo::default())));
        let week = skill.execute(&forecast(json!({"when": "на неделю в москве"})), &context()).await.unwrap();
        assert_eq!(
            week.data.as_deref(),
            Some("Москва, неделя: 2026-11-01: от +6 до +14, пасмурно, вероятность осадков 10%; 2026-11-02: от +4 до +9, небольшой дождь, вероятность осадков 80%.")
        );
        let here = skill.execute(&forecast(json!({"place": "55.7, 37.6"})), &context()).await.unwrap();
        assert!(here.data.unwrap().starts_with("Здесь: сейчас +12"));
        let nowhere = skill.execute(&forecast(json!({"place": "Нигде"})), &context()).await.unwrap();
        assert_eq!(nowhere.speech.as_deref(), Some("Не нашла город «Нигде»."));
        let mut no_zone = context();
        no_zone.timezone = None;
        let unknown = skill.execute(&forecast(json!({})), &no_zone).await.unwrap();
        assert_eq!(unknown.speech.as_deref(), Some("Не знаю, для какого города узнать погоду."));
        assert_eq!(describe_code(99.0), "гроза с градом");
        assert_eq!(describe_code(f64::NAN), "погода без описания");
    }

    #[tokio::test]
    async fn says_a_weather_code_open_meteo_has_no_data_for_as_none_not_as_clear() {
        let gaps = json!({
            "current": {"temperature_2m": 5, "weather_code": null},
            "daily": {
                "time": ["2026-11-01", "2026-11-02"],
                "weather_code": [3, null],
                "temperature_2m_max": [9, 9],
                "temperature_2m_min": [4, 4],
            },
        });
        let get = Arc::new(OpenMeteo { forecast: Some(gaps), ..OpenMeteo::default() });
        let skill = WeatherSkill::new(config(), Some(get));
        let tomorrow =
            skill.execute(&forecast(json!({"place": "Москва", "day": "tomorrow"})), &context()).await.unwrap();
        assert_eq!(tomorrow.data.as_deref(), Some("Москва, завтра: от +4 до +9, погода без описания."));
        let today = skill.execute(&forecast(json!({"place": "Москва"})), &context()).await.unwrap();
        assert_eq!(
            today.data.as_deref(),
            Some("Москва: сейчас +5, погода без описания; сегодня от +4 до +9, пасмурно.")
        );
        // `includes` does not coerce: a code in a string is none either.
        assert_eq!(describe_code(code(Some(&json!("3")))), "погода без описания");
        assert_eq!(describe_code(code(Some(&json!(3)))), "пасмурно");
    }
}
