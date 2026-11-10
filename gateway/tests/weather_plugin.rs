//! The built-in «Погода» (packages/weather/backend.js) in the gateway's own QuickJS, with a host whose geocoder and
//! forecast are fakes — no network. The fake geocoder answers as Open-Meteo's does: a query is a prefix of the names
//! it returns. A place named in its case («в Москве») is found by its dictionary form, the likeliest of its namesakes
//! is chosen, and a far-off prefix match is never taken: live, 10.11.2026, «Какая погода в Москве?» was answered
//! for «Москверо», a village in New Mexico, the geocoder's first answer for «Москве».

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use duowave::plugins::quickjs::QuickJs;
use duowave::plugins::runtime::{CallKind, HostApi, PluginCode, PluginInstance, PluginRuntime};
use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

/// A place the fake geocoder knows, as Open-Meteo (GeoNames, `language=ru`) has it: name, country code, country,
/// population, kind, coordinates; and an English name it is also found by.
type Known = (&'static str, &'static str, &'static str, Option<u64>, &'static str, f64, f64, Option<&'static str>);

const WORLD: &[Known] = &[
    ("Москва", "RU", "Россия", Some(10_381_222), "PPLC", 55.752, 37.618, Some("Moscow")),
    ("Москва", "US", "США", Some(25_060), "PPLA2", 46.732, -117.0, Some("Moscow")),
    ("Москва", "RU", "Россия", Some(17), "PPL", 57.15, 34.69, None),
    ("Москверо", "US", "США", Some(93), "PPLA2", 35.776, -103.957, None),
    ("Москвец", "UA", "Украина", None, "PPL", 48.12, 37.77, None),
    ("Санкт-Петербург", "RU", "Россия", Some(5_351_935), "PPLA", 59.939, 30.314, Some("Saint Petersburg")),
    ("Питер", "RU", "Россия", None, "PPL", 58.53, 56.78, None),
    ("Питер", "RU", "Россия", None, "PPL", 57.81, 42.51, None),
    ("Петербург", "US", "США", Some(32_477), "PPLA2", 37.228, -77.402, None),
    ("Казань", "RU", "Россия", Some(1_243_500), "PPLA", 55.789, 49.122, None),
    ("Казан", "TR", "Турция", Some(23_889), "PPLA2", 40.205, 32.683, None),
    ("Тверь", "RU", "Россия", Some(420_065), "PPLA", 56.858, 35.901, None),
    ("Тверия", "IL", "Израиль", Some(44_779), "PPL", 32.796, 35.53, None),
    ("Тула", "RU", "Россия", Some(482_873), "PPLA", 54.196, 37.618, None),
    ("Тула", "MX", "Мексика", Some(10_043), "PPLA2", 22.99, -99.71, None),
    ("Туле", "GL", "Гренландия", None, "AIRB", 76.531, -68.703, None),
    ("Туль", "FR", "Франция", Some(17_680), "PPLA3", 48.675, 5.891, None),
    ("Тулуза", "FR", "Франция", Some(511_684), "PPLA", 43.604, 1.444, None),
    ("Ярославль", "RU", "Россия", Some(608_722), "PPLA", 57.63, 39.874, None),
    ("Новосибирск", "RU", "Россия", Some(1_612_833), "PPLA", 55.042, 82.935, None),
    ("Новосибирское", "RU", "Россия", None, "PPL", 46.95, 142.4, None),
    ("Екатеринбург", "RU", "Россия", Some(1_495_066), "PPLA", 56.852, 60.612, None),
    ("Сочи", "RU", "Россия", Some(327_608), "PPLA2", 43.599, 39.726, None),
    ("Соча", "RU", "Россия", None, "PPL", 58.3, 56.5, None),
    ("Нижний Новгород", "RU", "Россия", Some(1_259_013), "PPLA", 56.329, 44.002, None),
    ("Нижний Тагил", "RU", "Россия", Some(361_883), "PPLA2", 57.919, 59.965, None),
    ("Ростов", "UA", "Украина", None, "PPL", 51.07, 34.15, None),
    ("Ростов-на-Дону", "RU", "Россия", Some(1_130_305), "PPLA", 47.231, 39.723, None),
    ("Ростово", "RU", "Россия", None, "PPL", 56.62, 35.1, None),
    ("Ростов Великий", "RU", "Россия", Some(31_039), "PPLA2", 57.186, 39.414, None),
    ("Париж", "FR", "Франция", Some(2_138_551), "PPLC", 48.853, 2.349, None),
    ("Париж", "US", "США", Some(24_782), "PPLA2", 33.661, -95.556, None),
    ("Париж", "RU", "Россия", Some(2_200), "PPL", 53.293, 60.098, None),
    ("Орёл", "RU", "Россия", Some(303_696), "PPLA", 52.97, 36.064, None),
    ("Орел", "UA", "Украина", Some(142), "PPL", 46.6, 30.2, None),
    ("Орлеан", "FR", "Франция", Some(116_238), "PPLA", 47.903, 1.909, None),
    ("Пушкин", "RU", "Россия", Some(92_889), "PPL", 59.714, 30.397, None),
    ("Пушкино", "RU", "Россия", Some(102_816), "PPLA2", 55.988, 37.83, None),
];

/// A name as the fake geocoder compares it: lower case, ё as е, a hyphen as a space.
fn plain(text: &str) -> String {
    text.to_lowercase().replace('ё', "е").replace('-', " ")
}

/// The host: the fake geocoder over `world`, a fake forecast, kv in memory (shared between loads), the plugin's
/// settings. It notes every geocoder query and every place a forecast was asked for.
struct Host {
    world: Vec<Known>,
    geocoder_down: AtomicBool,
    queries: Mutex<Vec<String>>,
    forecasts: Mutex<Vec<(f64, f64)>>,
    kv: Arc<Mutex<Map<String, Value>>>,
    settings: Map<String, Value>,
}

impl Host {
    fn new(world: &[Known]) -> Arc<Host> {
        Host::with(world, Arc::default(), Map::new())
    }

    fn with(world: &[Known], kv: Arc<Mutex<Map<String, Value>>>, settings: Map<String, Value>) -> Arc<Host> {
        Arc::new(Host {
            world: world.to_vec(),
            geocoder_down: AtomicBool::new(false),
            queries: Mutex::default(),
            forecasts: Mutex::default(),
            kv,
            settings,
        })
    }

    fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }

    fn last_forecast(&self) -> Option<(f64, f64)> {
        self.forecasts.lock().unwrap().last().copied()
    }

    /// Open-Meteo's geocoder: every place whose name (or English name) starts with the query, the exact ones first,
    /// then the bigger; at most `count`.
    fn geocode(&self, url: &url::Url) -> Value {
        let query = |key: &str| url.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned());
        let name = query("name").unwrap_or_default();
        let count: usize = query("count").and_then(|count| count.parse().ok()).unwrap_or(10);
        assert_eq!(query("language").as_deref(), Some("ru"), "names in Russian");
        self.queries.lock().unwrap().push(name.clone());
        let wanted = plain(&name);
        let mut found: Vec<&Known> = self
            .world
            .iter()
            .filter(|known| {
                plain(known.0).starts_with(&wanted) || known.7.is_some_and(|english| plain(english).starts_with(&wanted))
            })
            .collect();
        found.sort_by_key(|known| (plain(known.0) != wanted, std::cmp::Reverse(known.3.unwrap_or(0))));
        let results: Vec<Value> = found
            .into_iter()
            .take(count)
            .map(|(name, code, country, population, kind, latitude, longitude, _)| {
                let mut place = json!({
                    "name": name, "country_code": code, "country": country, "feature_code": kind,
                    "latitude": latitude, "longitude": longitude,
                });
                if let Some(population) = population {
                    place["population"] = json!(population);
                }
                place
            })
            .collect();
        if results.is_empty() {
            json!({ "generationtime_ms": 0.1 })
        } else {
            json!({ "results": results, "generationtime_ms": 0.1 })
        }
    }

    /// Open-Meteo's forecast: the same week anywhere.
    fn forecast(&self, url: &url::Url) -> Value {
        let number = |key: &str| {
            url.query_pairs().find(|(name, _)| name == key).and_then(|(_, value)| value.parse::<f64>().ok()).unwrap()
        };
        self.forecasts.lock().unwrap().push((number("latitude"), number("longitude")));
        let days = 7;
        json!({
            "current": { "temperature_2m": 1.2, "apparent_temperature": -2.0, "weather_code": 3, "wind_speed_10m": 4.0 },
            "daily": {
                "time": (0..days).map(|day| format!("2026-11-{:02}", 10 + day)).collect::<Vec<_>>(),
                "weather_code": vec![3; days],
                "temperature_2m_max": vec![4.0; days],
                "temperature_2m_min": vec![-1.0; days],
                "precipitation_probability_max": vec![20; days],
            }
        })
    }
}

impl HostApi for Host {
    fn fetch(&self, request: Value) -> BoxFuture<'static, Result<Value, String>> {
        let url = url::Url::parse(request["url"].as_str().unwrap()).unwrap();
        let answer = match url.host_str() {
            Some("geocoding-api.open-meteo.com") if self.geocoder_down.load(Ordering::SeqCst) => {
                json!({ "status": 503, "url": url.as_str(), "headers": {}, "body": "" })
            }
            Some("geocoding-api.open-meteo.com") => {
                json!({ "status": 200, "url": url.as_str(), "headers": {}, "body": self.geocode(&url).to_string() })
            }
            Some("api.open-meteo.com") => {
                json!({ "status": 200, "url": url.as_str(), "headers": {}, "body": self.forecast(&url).to_string() })
            }
            other => panic!("the plugin asked for {other:?}"),
        };
        Box::pin(async move { Ok(answer) })
    }
    fn kv_get(&self, key: &str) -> Result<Option<Value>, String> {
        Ok(self.kv.lock().unwrap().get(key).cloned())
    }
    fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
        self.kv.lock().unwrap().insert(key.into(), value);
        Ok(())
    }
    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.kv.lock().unwrap().remove(key);
        Ok(())
    }
    fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String> {
        Ok(self.kv.lock().unwrap().keys().filter(|key| key.starts_with(prefix)).cloned().collect())
    }
    fn say(&self, _text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }
    fn settings(&self) -> Map<String, Value> {
        self.settings.clone()
    }
    fn log(&self, _level: &str, _message: &str) {}
}

/// The package's file, read as the gateway lays it out.
fn package_file(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages/weather").join(name)).unwrap()
}

/// The plugin loaded on `host`, within its manifest's memory budget.
async fn start(host: &Arc<Host>) -> Arc<dyn PluginInstance> {
    let manifest: Value = serde_json::from_str(&package_file("manifest.json")).unwrap();
    let code = PluginCode {
        id: "weather".into(),
        version: manifest["version"].as_str().unwrap().into(),
        source: package_file("backend.js"),
        memory_mb: manifest["budget"]["memoryMb"].as_u64().unwrap(),
        ..PluginCode::default()
    };
    QuickJs.start(code, host.clone() as Arc<dyn HostApi>).await.expect("the weather plugin loads")
}

/// What the plugin says to a weather phrase whose `{when}` caught these words (normalized, as phrases give them).
async fn say(plugin: &Arc<dyn PluginInstance>, when: &str) -> String {
    let answer = plugin.call("forecast", json!({ "when": when, "request": {} }), CallKind::Voice).await;
    let answer = answer.unwrap_or_else(|error| panic!("«{when}»: {error}"));
    answer.get("say").unwrap_or(&answer).as_str().unwrap().to_string()
}

async fn tool(plugin: &Arc<dyn PluginInstance>, input: Value) -> String {
    let answer = plugin.call("weatherNow", input.clone(), CallKind::Tool).await;
    answer.unwrap_or_else(|error| panic!("{input}: {error}")).as_str().unwrap().to_string()
}

fn near(at: Option<(f64, f64)>, latitude: f64, longitude: f64) -> bool {
    at.is_some_and(|(lat, lon)| (lat - latitude).abs() < 0.01 && (lon - longitude).abs() < 0.01)
}

#[tokio::test]
async fn finds_a_city_named_in_its_case_by_its_dictionary_form_and_the_likeliest_of_its_namesakes() {
    let host = Host::new(WORLD);
    let plugin = start(&host).await;
    let cases: &[(&str, &str, f64, f64)] = &[
        ("в москве", "Москва:", 55.752, 37.618),
        ("в питере", "Санкт-Петербург:", 59.939, 30.314),
        ("в петербурге", "Санкт-Петербург:", 59.939, 30.314),
        ("в казани", "Казань:", 55.789, 49.122),
        ("в твери", "Тверь:", 56.858, 35.901),
        ("в туле", "Тула:", 54.196, 37.618),
        ("в ярославле", "Ярославль:", 57.63, 39.874),
        ("в новосибирске", "Новосибирск:", 55.042, 82.935),
        ("в екатеринбурге", "Екатеринбург:", 56.852, 60.612),
        ("в сочи", "Сочи:", 43.599, 39.726),
        ("в нижнем новгороде", "Нижний Новгород:", 56.329, 44.002),
        ("в нижнем тагиле", "Нижний Тагил:", 57.919, 59.965),
        ("в ростове-на-дону", "Ростов-на-Дону:", 47.231, 39.723),
        ("в ростове на дону", "Ростов-на-Дону:", 47.231, 39.723),
        // Not the Ukrainian hamlet named exactly «Ростов», nor «Ростово».
        ("в ростове", "Ростов-на-Дону:", 47.231, 39.723),
        // Not the village in the Chelyabinsk region, though Russia is preferred…
        ("в париже", "Париж:", 48.853, 2.349),
        // …unless a country is named.
        ("в париже в сша", "Париж:", 33.661, -95.556),
        // A vowel the case drops: «Орле» → «Орёл».
        ("в орле", "Орёл:", 52.97, 36.064),
        // «в Пушкине» is Пушкин, though Пушкино is bigger; «в Пушкино» is Пушкино.
        ("в пушкине", "Пушкин:", 59.714, 30.397),
        ("в пушкино", "Пушкино:", 55.988, 37.83),
        ("для москвы", "Москва:", 55.752, 37.618),
        ("под москвой", "Москва:", 55.752, 37.618),
        ("завтра в казани", "Казань, завтра:", 55.789, 49.122),
        ("в казани на неделю", "Казань, неделя:", 55.789, 49.122),
        ("в выходные в твери", "Тверь:", 56.858, 35.901),
        ("в эту субботу в сочи", "Сочи:", 43.599, 39.726),
    ];
    for &(when, starts, latitude, longitude) in cases {
        let before = host.queries().len();
        let said = say(&plugin, when).await;
        assert!(said.starts_with(starts), "«{when}»: {said}");
        assert!(near(host.last_forecast(), latitude, longitude), "«{when}»: forecast for {:?}", host.last_forecast());
        let asked = &host.queries()[before..];
        assert!(asked.len() <= 3, "«{when}»: {asked:?}");
        assert!(!asked.iter().any(|query| query == "москве"), "«{when}»: the case form itself is never looked up");
    }
    // The question as it was heard live, now answered for Moscow.
    assert!(say(&plugin, "в москве").await.starts_with("Москва: сейчас +1, пасмурно, ветер 4 м/с; сегодня от -1 до +4"));
}

#[tokio::test]
async fn never_answers_for_a_far_off_prefix_match_and_says_plainly_the_place_is_not_found() {
    // The geocoder of 10.11.2026 for «Москве»: only «Москверо» and «Москвец» begin so.
    let without_moscow: Vec<Known> = WORLD.iter().copied().filter(|known| known.0 != "Москва").collect();
    let host = Host::new(&without_moscow);
    let plugin = start(&host).await;
    assert_eq!(say(&plugin, "в москве").await, "Не нашла такое место: в москве.");
    assert_eq!(say(&plugin, "в мухосранске").await, "Не нашла такое место: в мухосранске.");
    assert_eq!(host.last_forecast(), None, "no forecast for a place not found");
    assert_eq!(tool(&plugin, json!({ "place": "Мухосранск" })).await, "Не нашла такое место: Мухосранск.");
}

#[tokio::test]
async fn looks_a_place_up_once_and_remembers_it_across_loads_the_old_versions_guesses_forgotten() {
    let kv: Arc<Mutex<Map<String, Value>>> = Arc::default();
    // 1.1.1 kept the geocoder's first answer for the case form itself.
    kv.lock().unwrap().insert(
        "place:москве".into(),
        json!({ "name": "Москверо", "latitude": 35.776, "longitude": -103.957 }),
    );
    let host = Host::with(WORLD, kv.clone(), Map::new());
    let plugin = start(&host).await;
    assert!(say(&plugin, "в москве").await.starts_with("Москва:"));
    assert_eq!(host.queries(), ["москв"], "one query: the stem finds «Москва» among its prefixes");
    assert!(!kv.lock().unwrap().contains_key("place:москве"), "the old guess is gone");
    assert!(say(&plugin, "завтра в москве").await.starts_with("Москва, завтра:"));
    assert_eq!(say(&plugin, "в мухосранске").await, "Не нашла такое место: в мухосранске.");
    assert_eq!(say(&plugin, "в мухосранске").await, "Не нашла такое место: в мухосранске.");
    assert_eq!(host.queries(), ["москв", "мухосранск"], "a place found or not is asked about once");
    plugin.stop();

    // Loaded again, on the same store: nothing more to ask.
    let again = Host::with(WORLD, kv.clone(), Map::new());
    let plugin = start(&again).await;
    assert!(say(&plugin, "в москве").await.starts_with("Москва:"));
    assert!(near(again.last_forecast(), 55.752, 37.618));
    assert!(again.queries().is_empty(), "{:?}", again.queries());
}

#[tokio::test]
async fn a_tool_and_the_usual_place_give_names_as_they_are_a_latin_one_too() {
    let host = Host::new(WORLD);
    let plugin = start(&host).await;
    assert!(tool(&plugin, json!({ "place": "Нижний Новгород" })).await.starts_with("Нижний Новгород:"));
    assert!(tool(&plugin, json!({ "place": "Ростов-на-Дону", "when": "tomorrow" })).await.starts_with("Ростов-на-Дону, завтра:"));
    assert!(tool(&plugin, json!({ "place": "Париж, США" })).await.starts_with("Париж:"));
    assert!(near(host.last_forecast(), 33.661, -95.556));
    assert!(tool(&plugin, json!({ "place": "в Твери" })).await.starts_with("Тверь:"));
    assert!(tool(&plugin, json!({ "place": "55.75, 37.62" })).await.starts_with("Здесь:"));
    // No place set: the station's time zone, its city in English.
    assert!(tool(&plugin, json!({ "timezone": "Europe/Moscow" })).await.starts_with("Москва:"));
    assert!(near(host.last_forecast(), 55.752, 37.618));

    // The place set in the plugin, for a phrase without one.
    let settings = json!({ "place": "Санкт-Петербург" }).as_object().cloned().unwrap();
    let own = Host::with(WORLD, Arc::default(), settings);
    let plugin = start(&own).await;
    assert!(say(&plugin, "сейчас").await.starts_with("Санкт-Петербург:"));
}

#[tokio::test]
async fn a_geocoder_that_does_not_answer_leaves_the_phrase_to_the_built_in_skill_and_is_not_remembered() {
    let host = Host::new(WORLD);
    host.geocoder_down.store(true, Ordering::SeqCst);
    let plugin = start(&host).await;
    let failed = plugin.call("forecast", json!({ "when": "в казани", "request": {} }), CallKind::Voice).await;
    assert!(failed.is_err(), "{failed:?}");
    host.geocoder_down.store(false, Ordering::SeqCst);
    assert!(say(&plugin, "в казани").await.starts_with("Казань:"));
}
