//! The talk without a station end to end (docs/plans/mic-mode.md): «Как говорить с DuoWave» and the lock on «Своё»,
//! a phrase answered by the brain in phrases with the tools of a conversation, its request in the journal. Each test
//! starts its own gateway on a free port and signs in from scratch; the model, the weather and the MCP server are
//! local fakes.

mod fakes;
mod support;

use fakes::ChatStub;
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, until, Gateway};

const ENV: &[(&str, &str)] = &[("UNIPROXY_MODE", "relay"), ("DEBUG_API_ENABLED", "true")];

trait Signed {
    fn signed(self, token: &str) -> Self;
}

impl Signed for reqwest::RequestBuilder {
    fn signed(self, token: &str) -> Self {
        let (name, value) = bearer(token);
        self.header(name, value)
    }
}

/// The first-run setup from this computer: the admin's token.
async fn setup(gateway: &Gateway) -> String {
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({ "password": "admin-password-1" })),
        StatusCode::CREATED,
    );
    setup.await["token"].as_str().expect("a token").to_string()
}

async fn mode(gateway: &Gateway, admin: &str) -> Value {
    message(gateway.get("/api/v1/station/mode").signed(admin), StatusCode::OK).await["mode"].clone()
}

#[tokio::test]
async fn the_microphone_locks_svoi_and_the_station_brings_the_mode_back() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway).await;
    let input = "/api/v1/talk/input";
    assert_eq!(message(gateway.get(input).signed(&admin), StatusCode::OK).await, json!({ "input": "station" }));
    let health = message(gateway.get("/health").signed(&admin), StatusCode::OK).await;
    assert_eq!((health["voiceInput"].clone(), health["panelMode"].clone()), (json!("station"), json!("relay")));

    let chosen = message(gateway.put(input).signed(&admin).json(&json!({ "input": "computer" })), StatusCode::OK).await;
    assert_eq!(chosen, json!({ "input": "computer", "before": "relay" }));
    assert_eq!(mode(&gateway, &admin).await, "own");
    let health = message(gateway.get("/health").signed(&admin), StatusCode::OK).await;
    assert_eq!((health["voiceInput"].clone(), health["panelMode"].clone()), (json!("computer"), json!("own")));

    // The panel's switch and the debug API's alike: only «Своё» while the microphone is chosen.
    for path in ["/api/v1/station/mode", "/debug/mode"] {
        for other in ["relay", "gateway"] {
            let (status, body) = send(gateway.post(path).signed(&admin).json(&json!({ "mode": other }))).await;
            assert_eq!(status, StatusCode::CONFLICT, "{path} {other}");
            assert_eq!(body["message"], duowave::talk::input::ONLY_OWN);
        }
    }
    assert_eq!(mode(&gateway, &admin).await, "own");

    // The choice is this computer's own: the package for the station's gateway has none of it.
    let bundle = duowave::placement::transfer::export(&gateway.app.db, &gateway.app.secrets, "pc").unwrap();
    let sent = serde_json::to_value(&bundle).unwrap();
    assert!(sent["settings"].get("voice_input").is_none(), "{}", sent["settings"]);

    let back = message(gateway.put(input).signed(&admin).json(&json!({ "input": "station" })), StatusCode::OK).await;
    assert_eq!(back, json!({ "input": "station" }));
    assert_eq!(mode(&gateway, &admin).await, "relay", "the mode from before");
    let free = send(gateway.post("/api/v1/station/mode").signed(&admin).json(&json!({ "mode": "gateway" }))).await;
    assert_eq!(free.0, StatusCode::CREATED, "the lock is off");

    let (status, body) = send(gateway.put(input).signed(&admin).json(&json!({ "input": "phone" }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, Some("input: station или computer")));
    gateway.stop().await;
}

/// A gateway with the stub model as its brain, and the admin's token.
async fn with_brain(llm: &ChatStub, extra: &[(&str, &str)]) -> (Gateway, String) {
    let mut vars = vec![("DIALOG_PROVIDER", "openai"), ("LLM_BASE_URL", llm.url.as_str()), ("LLM_MODEL", "test-llm")];
    vars.extend_from_slice(extra);
    let gateway = Gateway::start(&vars).await;
    let admin = setup(&gateway).await;
    (gateway, admin)
}

/// The events of a talk's stream, read to its end.
async fn events(request: reqwest::RequestBuilder) -> Vec<Value> {
    let response = request.header("accept", "text/event-stream").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream; charset=utf-8");
    let body = response.text().await.unwrap();
    body.split("\n\n")
        .filter_map(|chunk| chunk.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|event| event["type"].as_str().unwrap()).collect()
}

async fn journal(gateway: &Gateway, admin: &str) -> Vec<Value> {
    let journal = message(gateway.get("/api/v1/requests?device=computer").signed(admin), StatusCode::OK).await;
    journal["requests"].as_array().unwrap().clone()
}

fn tools_of(request: &Value) -> Vec<String> {
    let tools = request["tools"].as_array().cloned().unwrap_or_default();
    tools.iter().map(|tool| tool["function"]["name"].as_str().unwrap_or("").to_string()).collect()
}

#[tokio::test]
async fn a_phrase_is_answered_in_phrases_and_journaled_as_the_computers_request() {
    let llm = ChatStub::start().await;
    let (gateway, admin) = with_brain(&llm, &[]).await;
    llm.set_reply("Сейчас полдень. Хорошего дня!");

    let said = events(gateway.post("/api/v1/talk").signed(&admin).json(&json!({
        "text": "Который час?", "via": "button", "heard": { "by": "home", "ms": 420, "what": "gigaam-v3" }, "voice": true
    })))
    .await;
    assert_eq!(types(&said), ["start", "phrase", "phrase", "done"], "{said:?}");
    assert_eq!((said[0]["by"].clone(), said[1]["text"].clone()), (json!("home"), json!("Сейчас полдень.")));
    assert_eq!(said[2]["text"], "Хорошего дня!");
    assert_eq!(said[3]["said"], "Сейчас полдень. Хорошего дня!");

    // The request of the journal: the computer's, by voice, in «Своё», with the ear's, the brain's and the voice's steps.
    assert!(until(2000, || !gateway.app.requests.recent_views(5).is_empty()).await);
    let requests = journal(&gateway, &admin).await;
    let request = &requests[0];
    assert_eq!(request["id"], said[0]["request"]);
    assert_eq!((request["device"].as_str(), request["kind"].as_str()), (Some("computer"), Some("voice")));
    assert_eq!((request["mode"].as_str(), request["status"].as_str()), (Some("own"), Some("answered")));
    assert_eq!((request["text"].as_str(), request["executor"].as_str()), (Some("Который час?"), Some("model")));
    assert_eq!(request["answer"], "Сейчас полдень. Хорошего дня!");
    let steps: Vec<(&str, &str)> = request["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| (step["phase"].as_str().unwrap(), step["by"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(steps, [("recognize", "own"), ("think", "own"), ("speak", "own")]);
    assert_eq!(
        (request["steps"][0]["ms"].clone(), request["steps"][0]["what"].clone()),
        (json!(420), json!("gigaam-v3"))
    );
    assert_eq!(request["steps"][1]["what"], "test-llm");

    // The brain was told who it is on a computer: no word of a station in its instructions.
    let system = llm.requests()[0]["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(system.starts_with("Ты — ДиВи, голосовой помощник на компьютере"), "{system}");
    assert!(system.contains("Ты на компьютере:") && system.contains("Сейчас "), "{system}");
    for word in ["Алис", "колонк", "Яндекс"] {
        assert!(!system.contains(word), "{word} in the computer's prompt: {system}");
    }

    // Typed, and without a stream: the whole answer at once; the same talk as the check chat's (`panel:<person>`).
    llm.set_reply("Да, я здесь.");
    let whole =
        message(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Ты здесь?" })), StatusCode::OK)
            .await;
    assert_eq!(whole, json!({ "said": "Да, я здесь.", "by": "home" }));
    let asked = llm.requests().last().unwrap().clone();
    assert!(asked["messages"].to_string().contains("Который час?"), "the talk goes on: {}", asked["messages"]);
    let typed = journal(&gateway, &admin).await;
    assert_eq!((typed[0]["kind"].as_str(), typed[0]["steps"].as_array().unwrap().len()), (Some("text"), 1));

    // `fresh` starts the talk over.
    message(
        gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Заново", "fresh": true })),
        StatusCode::OK,
    )
    .await;
    assert!(!llm.requests().last().unwrap()["messages"].to_string().contains("Который час?"));

    let (status, body) = send(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": " " }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, Some("Скажите или напишите что-нибудь.")));
    assert_eq!(send(gateway.post("/api/v1/talk").json(&json!({ "text": "Привет" }))).await.0, StatusCode::UNAUTHORIZED);
    gateway.stop().await;
}

#[tokio::test]
async fn refuses_in_words_while_no_model_is_chosen() {
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    let (status, body) = send(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Привет" }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some(duowave::talk::NO_MODEL)));
    assert!(journal(&gateway, &admin).await.is_empty(), "nothing asked, nothing journaled");
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_weather_goes_through_the_gateways_own_skill_and_the_stations_commands_are_not_offered() {
    let llm = ChatStub::start().await;
    let forecast = fakes::start_forecast().await;
    let (gateway, admin) =
        with_brain(&llm, &[("WEATHER_LOCATION", "25.20,55.27"), ("WEATHER_FORECAST_URL", forecast.as_str())]).await;
    llm.set_tool_call(Some(("get_weather", json!({}))));
    llm.set_reply("На улице плюс десять.");

    let said =
        events(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Какая погода?", "heard": "cloud" })))
            .await;
    assert_eq!(types(&said), ["start", "tool", "phrase", "done"], "{said:?}");
    assert_eq!((said[1]["name"].clone(), said[2]["text"].clone()), (json!("Погода"), json!("На улице плюс десять.")));

    let requests = llm.requests();
    let offered = tools_of(&requests[0]);
    assert!(offered.contains(&"get_weather".to_string()) && offered.contains(&"get_news".to_string()), "{offered:?}");
    for station in ["set_volume", "set_timer", "play_music", "play_radio"] {
        assert!(!offered.contains(&station.to_string()), "{station} is the station's: {offered:?}");
    }
    let results = requests.last().unwrap()["messages"].to_string();
    assert!(
        results.contains("\"role\":\"tool\"") && results.contains("10"),
        "the skill's data at the model: {results}"
    );

    let request = &journal(&gateway, &admin).await[0];
    let steps: Vec<(&str, &str)> = request["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| (step["phase"].as_str().unwrap(), step["what"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(steps, [("recognize", ""), ("think", "test-llm"), ("tool", "Погода"), ("think", "test-llm")]);
    assert_eq!(request["steps"][0]["by"], "cloud");
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offers_by_voice_only_the_mcp_servers_open_to_the_voice() {
    let llm = ChatStub::start().await;
    let (gateway, admin) = with_brain(&llm, &[]).await;
    let url = fakes::mcp_server::start().await;
    let added = message(
        gateway
            .post("/api/v1/mcp/servers")
            .signed(&admin)
            .json(&json!({ "transport": "http", "title": "Fixture", "url": url })),
        StatusCode::CREATED,
    )
    .await;
    let id = added["id"].as_str().unwrap().to_string();
    let app = gateway.app.clone();
    let ready = || app.mcp.states().iter().any(|server| server.id == id && format!("{:?}", server.state) == "Ready");
    assert!(until(5000, ready).await, "the fixture server ready");
    let tool = format!("/api/v1/mcp/servers/{id}/tools/lookup");
    assert_eq!(send(gateway.put(&tool).signed(&admin).json(&json!({ "on": true }))).await.0, StatusCode::OK);

    // Open to the voice (as a new server is): the talk offers its tool, and a call of it reaches the server.
    llm.set_tool_call(Some(("mcp_fixture__lookup", json!({ "q": "кофе" }))));
    llm.set_reply("Нашлось про кофе.");
    let said =
        events(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "Что известно про кофе?" }))).await;
    assert!(types(&said).contains(&"tool") && said.iter().any(|event| event["name"] == "Fixture"), "{said:?}");
    let requests = llm.requests();
    assert!(tools_of(&requests[0]).contains(&"mcp_fixture__lookup".to_string()));
    assert!(requests.last().unwrap()["messages"].to_string().contains("Нашлось: кофе"));

    // Closed to the voice: the microphone hears the room, so the talk offers none of it — the check chat still does.
    let server = format!("/api/v1/mcp/servers/{id}");
    let closed = send(gateway.patch(&server).signed(&admin).json(&json!({ "voice": false }))).await;
    assert_eq!(closed.0, StatusCode::OK, "{}", closed.1);
    let before = llm.requests().len();
    events(gateway.post("/api/v1/talk").signed(&admin).json(&json!({ "text": "А про чай?", "heard": "home" }))).await;
    let offered = tools_of(&llm.requests()[before]);
    assert!(!offered.iter().any(|name| name.starts_with("mcp_")), "{offered:?}");
    gateway.stop().await;
}

/// A voice server of ours: answers every phrase with a third of a second of sound, a WAV at 22 kHz as Piper speaks.
async fn voice_server() -> String {
    use duowave::speech::audio::pcm::{encode_wav, resample};
    let wav = encode_wav(&resample(&duowave::speech::test_audio::voice(300.0, -20.0), 16000, 22050), 22050);
    let app = axum::Router::new().route(
        "/v1/audio/speech",
        axum::routing::post(move || {
            let wav = wav.clone();
            async move { ([("content-type", "audio/wav")], wav) }
        }),
    );
    format!("{}/v1", fakes::serve(app).await)
}

#[tokio::test]
async fn says_a_phrase_with_the_households_own_voice_as_a_wav_and_refuses_in_words_without_one() {
    // Alice's voice is chosen (the default): it comes through a station and cannot speak here.
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    let speech = "/api/v1/talk/speech";
    let (status, body) = send(gateway.post(speech).signed(&admin).json(&json!({ "text": "Привет." }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some(duowave::talk::routes::NO_VOICE)));
    gateway.stop().await;

    // Our own voice: a WAV, 16-bit mono, at the server's rate.
    let url = voice_server().await;
    let gateway = Gateway::start(&[("TTS_PROVIDER", "own"), ("TTS_OWN_URL", url.as_str())]).await;
    let admin = setup(&gateway).await;
    let response =
        gateway.post(speech).signed(&admin).json(&json!({ "text": "Сейчас полдень." })).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "audio/wav");
    let wav = response.bytes().await.unwrap();
    assert_eq!((&wav[0..4], &wav[8..12]), (&b"RIFF"[..], &b"WAVE"[..]));
    let word = |at: usize| u16::from_le_bytes([wav[at], wav[at + 1]]);
    assert_eq!((word(20), word(22), word(34)), (1, 1, 16), "PCM, mono, 16-bit");
    assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 22050);

    let (status, body) = send(gateway.post(speech).signed(&admin).json(&json!({ "text": "я".repeat(601) }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, Some("Фраза длиннее 600 знаков.")));
    assert_eq!(send(gateway.post(speech).signed(&admin).json(&json!({}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send(gateway.post(speech).json(&json!({ "text": "Привет." }))).await.0, StatusCode::UNAUTHORIZED);
    gateway.stop().await;

    // The voice server is not up (yet): said in words, not as a failure of the gateway.
    let gateway = Gateway::start(&[("TTS_PROVIDER", "own"), ("TTS_OWN_URL", "http://127.0.0.1:9/v1")]).await;
    let admin = setup(&gateway).await;
    let (status, body) = send(gateway.post(speech).signed(&admin).json(&json!({ "text": "Привет." }))).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some(duowave::talk::routes::VOICE_STARTING)));
    gateway.stop().await;
}

#[cfg(feature = "pc")]
#[tokio::test]
async fn the_shop_sends_no_card_of_yandex_while_the_microphone_is_chosen() {
    let gateway = Gateway::start(ENV).await;
    let admin = setup(&gateway).await;
    let cards = |shop: &Value| -> Vec<String> {
        shop["tts"].as_array().unwrap().iter().map(|card| card["id"].as_str().unwrap().to_string()).collect()
    };
    let shop = message(gateway.get("/api/v1/models").signed(&admin), StatusCode::OK).await;
    assert_eq!(cards(&shop)[0], "yandex", "the station's way: Alice's voice first");
    message(gateway.put("/api/v1/talk/input").signed(&admin).json(&json!({ "input": "computer" })), StatusCode::OK)
        .await;
    let shop = message(gateway.get("/api/v1/models").signed(&admin), StatusCode::OK).await;
    assert!(!cards(&shop).contains(&"yandex".to_string()), "{:?}", cards(&shop));
    assert!(cards(&shop).contains(&"silero".to_string()), "our own voices stay");
    assert_ne!(shop["ttsActive"]["modelId"], "yandex");
    // The built-in character goes by the talk's name there.
    let persona = message(gateway.get("/api/v1/persona").signed(&admin), StatusCode::OK).await;
    assert_eq!(persona["presets"][0]["title"], "ДиВи");
    gateway.stop().await;
}
