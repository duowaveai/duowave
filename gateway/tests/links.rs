//! The modules linked in App (src/links), end to end: a real gateway on a free port answers /health from the speech,
//! the dialog, the voice and the skills, lists its connectors over the dialog, the voice and the MCP servers in the
//! Node order, and its check chat («Спросить мозг») asks the gateway's own dialog. The echo model answers: no key, no
//! network.

mod support;

use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, Gateway};

const ENV: &[(&str, &str)] = &[("DIALOG_PROVIDER", "echo"), ("UNIPROXY_MODE", "gateway"), ("TTS_VOICE", "alice")];

/// The first admin, as the panel's first start makes one.
async fn admin(gateway: &Gateway) -> String {
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "links-admin-password"})),
        StatusCode::CREATED,
    );
    setup.await["token"].as_str().expect("a token").to_string()
}

#[tokio::test]
async fn health_tells_the_speech_the_dialog_the_voice_and_the_skills() {
    let gateway = Gateway::start(ENV).await;
    let health = message(gateway.get("/health"), StatusCode::OK).await;
    assert_eq!((health["status"].as_str(), health["mode"].as_str()), (Some("ok"), Some("gateway")));
    assert_eq!(health["speech"], json!({"vad": "energy", "asr": "off"}));
    assert_eq!(health["dialog"], "echo (repeats the question, no LLM)");
    assert_eq!(health["brain"]["provider"], "echo");
    assert_eq!(health["tts"], "yandex via wss://uniproxy.alice.yandex.net/uni.ws, voice alice");
    let skills = &health["skills"];
    for part in ["routes", "implemented", "served", "unsupported"] {
        assert!(skills[part].is_object(), "skills.{part}: {skills}");
    }
    assert_eq!(skills["implemented"]["weather"], true);
    gateway.stop().await;
}

#[tokio::test]
async fn lists_the_connectors_in_the_node_order_with_the_models_and_the_mcp_servers() {
    let gateway = Gateway::start(ENV).await;
    let token = admin(&gateway).await;
    let (name, value) = bearer(&token);
    let added = message(
        gateway
            .post("/api/v1/mcp/servers")
            .header(name.clone(), value.clone())
            .json(&json!({"transport": "http", "title": "Файлы", "url": "http://127.0.0.1:9/mcp", "enabled": false})),
        StatusCode::CREATED,
    )
    .await;
    let listed = message(gateway.get("/api/v1/connectors").header(name, value), StatusCode::OK).await;
    let connectors = listed["connectors"].as_array().unwrap();
    let ids: Vec<&str> = connectors.iter().map(|connector| connector["manifest"]["id"].as_str().unwrap()).collect();
    let mcp = format!("mcp:{}", added["id"].as_str().unwrap());
    assert_eq!(
        ids,
        [
            "yandex",
            "llm",
            "asr",
            "tts",
            "home-assistant",
            "telegram",
            "livekit",
            "sip",
            "weather",
            "news",
            "piper-registry",
            "plugins",
            mcp.as_str()
        ]
    );
    let status = |id: &str| connectors.iter().find(|c| c["manifest"]["id"] == id).unwrap()["status"].clone();
    assert_eq!(status("yandex"), json!({"state": "configured", "detail": "сессия откроется с колонкой"}));
    assert_eq!(status("llm"), json!({"state": "configured", "detail": "echo (repeats the question, no LLM)"}));
    assert_eq!(status("tts"), json!({"state": "unset", "detail": "свой голосовой сервер не задан"}));
    assert_eq!(status("news"), json!({"state": "unset", "detail": "лент нет"}));
    assert_eq!(status(&mcp)["state"], "off");
    assert_eq!(status("plugins"), json!({"state": "unset", "detail": "найдено 0, включённых нет"}));
    gateway.stop().await;
}

#[tokio::test]
async fn the_check_chat_asks_the_gateways_own_dialog() {
    let gateway = Gateway::start(ENV).await;
    let token = admin(&gateway).await;
    let (name, value) = bearer(&token);
    let response = gateway
        .post("/api/v1/brain/ask")
        .header(name, value)
        .json(&json!({"question": "Ты здесь?", "fresh": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream; charset=utf-8");
    let text = response.text().await.unwrap();
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|chunk| chunk.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    let kinds: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["start", "text", "done"], "{text}");
    assert_eq!(events[0]["tools"], 0);
    assert_eq!(events[0]["provider"], "Эхо");
    assert_eq!(events[1]["text"], "Вы сказали: Ты здесь?");
    gateway.stop().await;
}
