//! Plugins end to end (docs/plans/plugins.md, stages 2, 4 and 5): «Радио» is built into the gateway (packages/radio,
//! packages/bundled.json) and laid out into its DATA_DIR at the start, on; a station of the test harness saying «включи
//! радио джаз» — the plugin's phrase, its function, our player's audio_play. No station, no network: the station is
//! the harness's, and the plugin finds the station in its own list.

mod fakes;
mod support;

use std::path::Path;

use fakes::gateway_yandex::GatewayYandex;
use fakes::{at, StationClient};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, status, Gateway};

const STATION_HEADERS: &[(&str, &str)] = &[("user-agent", "WebSocket++/0.8.2"), ("x-uprx-oauth-token", "y0_test")];

/// The built-in «Радио»'s version: the repository's manifest.
fn radio_version() -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages/radio/manifest.json");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    manifest["version"].as_str().unwrap().to_string()
}

struct Env {
    gateway: Gateway,
    admin: String,
    _yandex: GatewayYandex,
}

async fn env(extra: &[(&str, &str)]) -> Env {
    let yandex = GatewayYandex::start().await;
    let yandex_url = yandex.url();
    let mut vars: Vec<(&str, &str)> = vec![
        ("UNIPROXY_MODE", "gateway"),
        ("YANDEX_UNIPROXY_URL", &yandex_url),
        ("STATION_PAIRING", "open"),
        ("DEBUG_API_ENABLED", "true"),
        ("RECORDER_ENABLED", "true"),
        ("PLUGINS_BUNDLED", "true"),
    ];
    vars.extend_from_slice(extra);
    let gateway = Gateway::start(&vars).await;
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "plugins-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    Env { gateway, admin, _yandex: yandex }
}

impl Env {
    async fn get(&self, path: &str) -> Value {
        let (name, value) = bearer(&self.admin);
        message(self.gateway.get(path).header(name, value), StatusCode::OK).await
    }

    async fn post(&self, path: &str, token: &str) -> (StatusCode, Value) {
        let (name, value) = bearer(token);
        send(self.gateway.post(path).header(name, value).json(&json!({}))).await
    }

    async fn station(&self, sync_id: &str) -> StationClient {
        let url = format!("ws://127.0.0.1:{}/uni.ws", self.gateway.port);
        let station = StationClient::connect(&url, STATION_HEADERS).await.expect("the station connects");
        station.synchronize(sync_id).await;
        station
    }

    /// The route the journal noted for a text.
    async fn route(&self, text: &str) -> Value {
        for _ in 0..40 {
            for session in self.get("/debug/sessions").await.as_array().unwrap() {
                let path = format!("/debug/sessions/{}/events?limit=500", session["id"].as_str().unwrap());
                let events = self.get(&path).await;
                if let Some(route) =
                    events.as_array().unwrap().iter().find(|e| e["kind"] == "route" && e["data"]["text"] == text)
                {
                    return route["data"].clone();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no route for {text}");
    }
}

fn response_to(request_id: &str) -> impl Fn(&Value) -> bool + '_ {
    move |m| at(m, "event.aliceResponse.header.requestId") == request_id
}

/// A protobuf Struct as the station's codec shows it (`fields`, `stringValue`, …) as plain JSON.
fn plain(value: &Value) -> Value {
    if let Some(fields) = value.get("fields").and_then(Value::as_object) {
        return Value::Object(fields.iter().map(|(key, field)| (key.clone(), plain(field))).collect());
    }
    for kind in ["stringValue", "boolValue", "numberValue"] {
        if let Some(found) = value.get(kind) {
            return found.clone();
        }
    }
    if let Some(inner) = value.get("structValue") {
        return plain(inner);
    }
    if let Some(items) = value.pointer("/listValue/values").and_then(Value::as_array) {
        return Value::Array(items.iter().map(plain).collect());
    }
    value.clone()
}

fn directives(message: &Value) -> Vec<Value> {
    at(message, "event.aliceResponse.response.directives").as_array().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_the_pilots_radio_when_the_station_says_vklyuchi_radio_and_leaves_stop_to_the_built_in_player() {
    let env = env(&[]).await;
    let version = radio_version();
    // Built in: laid out at the start and on, its code not loaded until it is called; never removed.
    let listed = env.get("/api/v1/packages").await;
    assert_eq!(listed["safeMode"], false);
    let radio = listed["packages"].as_array().unwrap().iter().find(|p| p["id"] == "radio").expect("radio is built in");
    assert_eq!(
        (&radio["enabled"], &radio["state"], &radio["origin"]),
        (&json!(true), &json!("ready"), &json!("bundled"))
    );
    assert_eq!(
        (&radio["removable"], &radio["canDisable"], &radio["version"]),
        (&json!(false), &json!(true), &json!(version))
    );
    assert_eq!(
        radio["permissions"],
        json!(["net:*.api.radio-browser.info", "voice.intents", "llm.tools", "media.source", "kv", "events"])
    );
    assert_eq!(radio["budget"], json!({"memoryMb": 4, "storageMb": 1}));
    assert!(radio["panelModule"].as_str().is_some_and(|module| module.ends_with("/panel.js")), "{radio}");
    let (code, refused) =
        send(env.gateway.delete("/api/v1/packages/radio").header(bearer(&env.admin).0, bearer(&env.admin).1)).await;
    assert_eq!(code, StatusCode::CONFLICT, "a built-in is not removed: {refused}");

    let station = env.station("sync-radio").await;
    station.text_input("text-radio", "req-radio", json!({ "type": "text_input", "text": "Включи радио джаз" }));
    let response = station.wait_for(response_to("req-radio"), 8000).await;
    let play = directives(&response).into_iter().find(|d| d["name"] == "audio_play").expect("audio_play");
    let payload = plain(&play["payload"]);
    assert_eq!(payload["stream"]["url"], "https://main.hostingradio.ru/jazz-128.mp3");
    assert_eq!(payload["stream"]["type"], "FmRadio");
    assert_eq!(payload["stream"]["id"], "radio/jazz");
    assert_eq!(payload["callbacks"]["on_started"]["name"], "vgw_media_on_started", "our player's callbacks");
    let route = env.route("Включи радио джаз").await;
    assert_eq!(
        (&route["capability"], &route["executor"], &route["source"], &route["detail"]),
        (&json!("radio"), &json!("own"), &json!("quick"), &json!("plugin:radio"))
    );

    // «Стоп» stays the built-in player's rule, whatever plays.
    station.text_input("text-stop", "req-stop", json!({ "type": "text_input", "text": "Стоп" }));
    station.wait_for(response_to("req-stop"), 8000).await;
    let stop = env.route("Стоп").await;
    assert_eq!((&stop["capability"], &stop["detail"]), (&json!("playback"), &json!("stop")));

    let view = env.get("/api/v1/packages/radio").await;
    assert_eq!((&view["calls"], &view["errors"], &view["loaded"]), (&json!(1), &json!(0), &json!(true)));
    assert!(view["memory"]["used"].as_u64().unwrap() > 0, "{view}");
    station.close();

    // A file of the package's assets/ is our /media's, only inside that folder and only while the plugin is on.
    let assets = env.gateway.dir.path().join(format!("packages/radio/{version}/assets"));
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("jingle.mp3"), b"ID3 jingle").unwrap();
    let media = |path: &str| env.gateway.get(path);
    let jingle = media("/media/packages/radio/jingle.mp3").send().await.unwrap();
    assert_eq!(jingle.status(), StatusCode::OK);
    assert_eq!(&jingle.bytes().await.unwrap()[..], b"ID3 jingle");
    assert_eq!(status(media("/media/packages/radio/..%2Fmanifest.json")).await, StatusCode::NOT_FOUND);
    assert_eq!(status(media("/media/packages/radio/backend.js")).await, StatusCode::NOT_FOUND);
    assert_eq!(env.post("/api/v1/packages/radio/disable", &env.admin).await.0, StatusCode::OK);
    assert_eq!(status(media("/media/packages/radio/jingle.mp3")).await, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lets_only_the_admin_switch_plugins_and_shows_them_to_whoever_sees_the_connectors() {
    let env = env(&[]).await;
    let add = env
        .gateway
        .post("/api/v1/users")
        .header(bearer(&env.admin).0, bearer(&env.admin).1)
        .json(&json!({"name": "Аня", "password": "anya-password", "role": "member"}));
    assert_eq!(status(add).await, StatusCode::CREATED);
    let login = message(
        env.gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"})),
        StatusCode::OK,
    )
    .await;
    let member = login["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&member);
    assert_eq!(status(env.gateway.get("/api/v1/packages").header(name, value)).await, StatusCode::OK);
    assert_eq!(env.post("/api/v1/packages/radio/enable", &member).await.0, StatusCode::FORBIDDEN);
    assert_eq!(status(env.gateway.get("/api/v1/packages")).await, StatusCode::UNAUTHORIZED);
    assert_eq!(env.post("/api/v1/packages/nothing/enable", &env.admin).await.0, StatusCode::NOT_FOUND);
    // The tool, tried as «Мозг» would call it: offline, from its own list.
    assert_eq!(env.post("/api/v1/packages/radio/enable", &env.admin).await.0, StatusCode::OK);
    let (name, value) = bearer(&env.admin);
    let tried = message(
        env.gateway
            .post("/api/v1/packages/radio/tools/radio_search/try")
            .header(name, value)
            .json(&json!({"arguments": {"query": "классика"}})),
        StatusCode::OK,
    )
    .await;
    let found: Value = serde_json::from_str(tried["content"].as_str().unwrap()).unwrap();
    let titles: Vec<&str> =
        found["stations"].as_array().unwrap().iter().map(|s| s["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Радио Орфей", "Радио Культура"]);
    // The connectors list shows the plugins as one connector.
    let connectors = env.get("/api/v1/connectors").await;
    let plugins =
        connectors["connectors"].as_array().unwrap().iter().find(|c| c["manifest"]["id"] == "plugins").cloned();
    let plugins = plugins.expect("the plugins' connector");
    assert_eq!((&plugins["manifest"]["kind"], &plugins["status"]["state"]), (&json!("plugins"), &json!("ready")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_no_plugin_in_the_safe_mode() {
    let env = env(&[("PLUGINS_DISABLED", "1")]).await;
    let listed = env.get("/api/v1/packages").await;
    assert_eq!(listed["safeMode"], true);
    assert_eq!(listed["packages"][0]["state"], "safeMode");
    assert_eq!(env.post("/api/v1/packages/radio/enable", &env.admin).await.0, StatusCode::CONFLICT);
    // The phrase is the built-in radio's again.
    let station = env.station("sync-safe").await;
    station.text_input("text-safe", "req-safe", json!({ "type": "text_input", "text": "Включи радио джаз" }));
    station.wait_for(response_to("req-safe"), 8000).await;
    assert_eq!(env.route("Включи радио джаз").await["detail"], "radio");
    station.close();
}

/// A live check of a gateway that runs (not a station's — a test one): the harness's station says LIVE_PHRASES
/// (split by `|`) to LIVE_GATEWAY and prints what came back.
///
/// `LIVE_GATEWAY=ws://127.0.0.1:8097/uni.ws cargo test -p duowave --test plugins live -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a running gateway: LIVE_GATEWAY"]
async fn live() {
    let url = std::env::var("LIVE_GATEWAY").expect("LIVE_GATEWAY=ws://<host>:<port>/uni.ws");
    let phrases = std::env::var("LIVE_PHRASES").unwrap_or_else(|_| "Включи радио джаз".into());
    let station = StationClient::connect(&url, STATION_HEADERS).await.expect("the station connects");
    station.synchronize("live-sync").await;
    for (n, phrase) in phrases.split('|').enumerate() {
        let (message, request) = (format!("live-{n}"), format!("req-live-{n}"));
        let started = std::time::Instant::now();
        station.text_input(&message, &request, json!({ "type": "text_input", "text": phrase }));
        let response = station.wait_for(response_to(&request), 15_000).await;
        let commands: Vec<Value> = directives(&response)
            .iter()
            .map(|directive| {
                let payload = plain(&directive["payload"]);
                json!({ "name": directive["name"], "title": payload["metadata"]["title"], "url": payload["stream"]["url"] })
            })
            .collect();
        println!(
            "«{phrase}» → {} ms, speech {}, directives {}",
            started.elapsed().as_millis(),
            at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text"),
            Value::Array(commands)
        );
    }
    station.close();
}
