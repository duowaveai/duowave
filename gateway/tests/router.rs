//! The router's journal of deals end to end (docs/plans/router-and-works.md, stage 1). An answer of a stub model
//! leaves one deal: its seat, its reason, its tokens and its dollars at the snapshot's price, with the journal's
//! request it belongs to. A model at home costs 0 (`local`); a plan's sign-in is `plan`, priced as its key would be; a
//! price written by hand counts from the next deal; the days add up to the deals; old deals go, their days stay; only
//! the admin reads them. The journal's request shows its deal's seat and dollars; the panel reads a model's price,
//! whether it is written and what taking it away brings back. The station's answers carry the reason of the rule that
//! chose their model. Each test starts its own gateway on a free port; the models and Yandex are local fakes.

mod fakes;
mod support;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::post;
use duowave::config::DialogProvider;
use duowave::core::events::GatewayEvent;
use duowave::router::deals::{Deal, VIA_BRAIN};
use duowave::router::{Answerer, Links, Outcome, Reason, Seat, Spent, Tokens};
use fakes::gateway_yandex::GatewayYandex;
use fakes::{at, ChatStub, StationClient};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, status, until, Gateway};

const DAY_MS: u64 = 86_400_000;
/// The model of the stub Claude: one the snapshot prices.
const CLAUDE_MODEL: &str = "claude-haiku-5-5";

trait Signed {
    fn signed(self, token: &str) -> Self;
}

impl Signed for reqwest::RequestBuilder {
    fn signed(self, token: &str) -> Self {
        let (name, value) = bearer(token);
        self.header(name, value)
    }
}

async fn setup(gateway: &Gateway) -> String {
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({ "password": "admin-password-1" })),
        StatusCode::CREATED,
    );
    setup.await["token"].as_str().expect("a token").to_string()
}

/// Anthropic's streaming Messages API, enough for the gateway's client: one text block, 12 tokens in, 2000 read from
/// the cache, 3 out. Counts the answers.
#[derive(Clone)]
struct ClaudeStub {
    url: String,
    asked: Arc<Mutex<usize>>,
}

impl ClaudeStub {
    async fn start() -> ClaudeStub {
        let asked = Arc::new(Mutex::new(0));
        let app = axum::Router::new().route("/v1/messages", post(claude)).with_state(asked.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        ClaudeStub { url, asked }
    }
}

fn event(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

async fn claude(State(asked): State<Arc<Mutex<usize>>>) -> axum::response::Response {
    *asked.lock().unwrap() += 1;
    let body = [
        event(
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant",
                "model": CLAUDE_MODEL, "content": [], "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 12, "output_tokens": 1, "cache_read_input_tokens": 2000}}}),
        ),
        event("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Сейчас полдень."}}),
        ),
        event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        event(
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 3}}),
        ),
        event("message_stop", json!({"type": "message_stop"})),
    ]
    .concat();
    ([(CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// A gateway whose model is Claude in the cloud (the stub at its address), and the admin's token.
async fn with_claude(claude: &ClaudeStub) -> (Gateway, String) {
    let gateway = Gateway::start(&[
        ("DIALOG_PROVIDER", "anthropic"),
        ("LLM_BASE_URL", claude.url.as_str()),
        ("LLM_API_KEY", "test-key"),
        ("LLM_MODEL", CLAUDE_MODEL),
    ])
    .await;
    let admin = setup(&gateway).await;
    (gateway, admin)
}

/// A typed phrase to the computer's talk, answered whole: the request's id from the journal.
async fn talk(gateway: &Gateway, admin: &str, text: &str) -> String {
    let before = gateway.app.requests.recent_views(50).len();
    message(gateway.post("/api/v1/talk").signed(admin).json(&json!({ "text": text })), StatusCode::OK).await;
    assert!(until(3000, || gateway.app.requests.recent_views(50).len() > before).await, "the request ended");
    gateway.app.requests.recent_views(1)[0].id.clone()
}

async fn deals(gateway: &Gateway, admin: &str, query: &str) -> Vec<Value> {
    let listed = message(gateway.get(&format!("/api/v1/router/deals{query}")).signed(admin), StatusCode::OK).await;
    listed["deals"].as_array().unwrap().clone()
}

#[tokio::test]
async fn an_answer_of_a_cloud_model_is_a_deal_with_its_seat_reason_tokens_and_dollars_at_the_snapshots_price() {
    let claude = ClaudeStub::start().await;
    let (gateway, admin) = with_claude(&claude).await;
    let mut told = gateway.app.events.subscribe();

    let request = talk(&gateway, &admin, "Который час?").await;
    assert_eq!(*claude.asked.lock().unwrap(), 1);
    let listed = deals(&gateway, &admin, "").await;
    assert_eq!(listed.len(), 1, "one answer, one deal: {listed:?}");
    let deal = &listed[0];
    // The computer's talk answers on «Быстрая» as it stands: no rule chose it.
    assert_eq!((deal["seat"].as_str(), deal["reason"].as_str()), (Some("fast"), Some("seat")));
    assert_eq!((deal["provider"].as_str(), deal["model"].as_str()), (Some("env"), Some(CLAUDE_MODEL)));
    assert_eq!((deal["via"].as_str(), deal["place"].as_str()), (Some(VIA_BRAIN), Some("cloud")));
    assert_eq!((deal["billing"].as_str(), deal["outcome"].as_str()), (Some("key"), Some("ok")));
    assert_eq!(deal["tokens"], json!({"input": 12, "output": 3, "cacheRead": 2000, "cacheWrite": 0}));
    assert_eq!(deal["rounds"], 1);
    assert_eq!(deal["request"].as_str(), Some(request.as_str()), "the journal's request it belongs to");
    // The dollars at the snapshot's price of the model, and that price kept in the deal.
    let price = gateway.app.router.pool.price("env", Some("claude"), CLAUDE_MODEL).expect("the snapshot prices it");
    let tokens = Tokens { input: 12, output: 3, cache_read: 2000, cache_write: 0 };
    assert_eq!(deal["usdMicro"].as_u64(), price.cost(&tokens));
    assert!(deal["usdMicro"].as_u64().unwrap() > 0);
    assert_eq!(deal["price"]["from"], "snapshot");
    assert_eq!(deal["price"]["input"].as_f64(), price.input);
    // No word of what was asked or answered.
    let text = deal.to_string();
    assert!(!text.contains("Который") && !text.contains("полдень"), "{text}");
    // The journal's request has the same tokens, by the same model.
    let journal = gateway.app.requests.recent_views(1);
    let spent = journal[0].tokens.as_ref().expect("the request's tokens");
    assert_eq!((spent.model.as_str(), spent.cache_read, spent.rounds), (CLAUDE_MODEL, 2000, 1));
    // And the deal's seat, the way it is paid and its dollars.
    assert_eq!(
        (spent.seat.as_deref(), spent.billing.as_deref(), spent.usd_micro),
        (Some("fast"), Some("key"), deal["usdMicro"].as_u64())
    );
    // Told: `router.deal`.
    let id = deal["id"].as_i64().unwrap();
    let mut heard = None;
    while let Ok(Ok(event)) = tokio::time::timeout(std::time::Duration::from_secs(2), told.recv()).await {
        if let GatewayEvent::RouterDeal { id: deal_id, seat, model, usd_micro, .. } = event.as_ref() {
            heard = Some((*deal_id, seat.clone(), model.clone(), *usd_micro));
            break;
        }
    }
    assert_eq!(heard, Some((id, "fast".to_string(), CLAUDE_MODEL.to_string(), price.cost(&tokens))));

    // The price as the panel reads it before writing one: the snapshot's, nothing written.
    let path = format!("/api/v1/router/prices/env/{CLAUDE_MODEL}");
    let now = message(gateway.get(&path).signed(&admin), StatusCode::OK).await;
    assert_eq!((now["price"]["from"].as_str(), now["written"].as_bool()), (Some("snapshot"), Some(false)));
    assert_eq!((now["unwritten"]["from"].as_str(), now["plan"].as_bool()), (Some("snapshot"), Some(false)));
    assert_eq!(now["price"]["input"].as_f64(), price.input);

    // A price written by hand counts from the next deal; the deal before keeps the price it was counted at.
    let written = message(
        gateway.put(&format!("/api/v1/router/prices/env/{CLAUDE_MODEL}")).signed(&admin).json(&json!({
            "input": 1, "output": 2, "context": 200000
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(written["price"]["from"], "manual");
    assert_eq!(written["context"], 200000);
    // Written: what taking it away brings back is the snapshot's.
    assert_eq!((written["written"].as_bool(), written["unwritten"]["from"].as_str()), (Some(true), Some("snapshot")));
    let now = message(gateway.get(&path).signed(&admin), StatusCode::OK).await;
    assert_eq!((now["written"].as_bool(), now["price"]["input"].as_f64()), (Some(true), Some(1.0)));
    talk(&gateway, &admin, "А сейчас?").await;
    let listed = deals(&gateway, &admin, "").await;
    assert_eq!(listed.len(), 2);
    // 12 × 1 + 3 × 2 + 2000 × 1 (the cache read without a price of its own costs as the input).
    assert_eq!((listed[0]["usdMicro"].as_u64(), listed[0]["price"]["from"].as_str()), (Some(2018), Some("manual")));
    assert_eq!(listed[1]["price"]["from"], "snapshot", "the older deal keeps its price");
    let back = message(
        gateway.put(&format!("/api/v1/router/prices/env/{CLAUDE_MODEL}")).signed(&admin).json(&json!({})),
        StatusCode::OK,
    )
    .await;
    assert_eq!((back["price"]["from"].as_str(), back["written"].as_bool()), (Some("snapshot"), Some(false)));
    // A model with slashes in its name, as OpenRouter writes them.
    let slashed = message(
        gateway.put("/api/v1/router/prices/openrouter/anthropic/claude-x").signed(&admin).json(&json!({
            "input": 3, "output": 15
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        (slashed["model"].as_str(), slashed["price"]["from"].as_str()),
        (Some("anthropic/claude-x"), Some("manual"))
    );
    let (code, body) =
        send(gateway.put("/api/v1/router/prices/env/x").signed(&admin).json(&json!({"input": -1, "output": 1}))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");

    // The days add up to the deals.
    let stats = message(gateway.get("/api/v1/router/stats?by=seat,model").signed(&admin), StatusCode::OK).await;
    let listed = deals(&gateway, &admin, "").await;
    let usd: u64 = listed.iter().map(|deal| deal["usdMicro"].as_u64().unwrap_or(0)).sum();
    assert_eq!(stats["by"], json!(["seat", "model"]));
    assert_eq!(stats["rows"].as_array().unwrap().len(), 1);
    assert_eq!(stats["rows"][0]["of"], json!({"seat": "fast", "model": CLAUDE_MODEL}));
    assert_eq!((stats["total"]["deals"].as_u64(), stats["total"]["usdMicro"].as_u64()), (Some(2), Some(usd)));
    assert_eq!(stats["total"]["tokens"]["cacheRead"], 4000);
    let (code, body) = send(gateway.get("/api/v1/router/stats?by=colour").signed(&admin)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    gateway.stop().await;
}

#[tokio::test]
async fn a_model_at_home_costs_nothing_and_a_plans_sign_in_is_priced_as_its_key() {
    let llm = ChatStub::start().await;
    let gateway =
        Gateway::start(&[("DIALOG_PROVIDER", "openai"), ("LLM_BASE_URL", llm.url.as_str()), ("LLM_MODEL", "test-llm")])
            .await;
    let admin = setup(&gateway).await;
    llm.set_reply("Сейчас полдень.");
    let request = talk(&gateway, &admin, "Который час?").await;
    let listed = deals(&gateway, &admin, "").await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    let deal = &listed[0];
    assert_eq!((deal["place"].as_str(), deal["billing"].as_str()), (Some("home"), Some("local")));
    assert_eq!((deal["usdMicro"].as_u64(), deal["price"]["from"].as_str()), (Some(0), Some("local")));
    // OpenAI's prompt tokens hold the cached ones: the deal's input is the fresh part.
    assert_eq!(deal["tokens"], json!({"input": 20, "output": 20, "cacheRead": 80, "cacheWrite": 0}));
    assert_eq!(deal["request"].as_str(), Some(request.as_str()));

    // ChatGPT's plan: `plan`, the dollars as OpenAI's key would have cost.
    message(
        gateway.post("/api/v1/brain/providers").signed(&admin).json(&json!({"preset": "chatgpt"})),
        StatusCode::CREATED,
    )
    .await;
    let spent = Spent {
        key: String::new(),
        by: Answerer {
            provider: "chatgpt".into(),
            model: "gpt-5".into(),
            kind: DialogProvider::OpenAi,
            cloud: true,
            seat: Seat::Smart,
            reason: Reason::RuleHard,
        },
        tokens: Tokens { input: 1000, output: 100, cache_read: 0, cache_write: 0 },
        rounds: 1,
        ms: 900,
        outcome: Outcome::Ok,
    };
    let plan = gateway.app.router.close(&spent, Links::default()).expect("written");
    let by_key = gateway.app.router.pool.price("openai", Some("openai"), "gpt-5").expect("the snapshot prices it");
    assert_eq!((plan.billing.as_str(), plan.place.as_str()), ("plan", "cloud"));
    assert_eq!(plan.usd_micro, by_key.cost(&spent.tokens));
    // A model no list knows by this exact name: no dollars, not a «similar» one's.
    let unknown = Spent { by: Answerer { model: "gpt-5-latest-preview".into(), ..spent.by.clone() }, ..spent.clone() };
    let unpriced = gateway.app.router.close(&unknown, Links::default()).unwrap();
    assert_eq!((unpriced.usd_micro, unpriced.price), (None, None));
    let stats = message(gateway.get("/api/v1/router/stats?by=billing").signed(&admin), StatusCode::OK).await;
    let billing: Vec<(&str, u64)> = stats["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["of"]["billing"].as_str().unwrap(), row["deals"].as_u64().unwrap()))
        .collect();
    assert!(billing.contains(&("plan", 2)) && billing.contains(&("local", 1)), "{billing:?}");
    assert_eq!(stats["total"]["unpriced"], 1);
    gateway.stop().await;
}

#[tokio::test]
async fn old_deals_go_their_days_stay_and_only_the_admin_reads_them() {
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    let now = duowave::utils::time::now_ms();
    let old = |age_days: u64, model: &str| Deal {
        id: 0,
        at: now - age_days * DAY_MS,
        seat: "fast".into(),
        via: VIA_BRAIN.into(),
        provider: "env".into(),
        model: model.into(),
        place: "cloud".into(),
        reason: "seat".into(),
        request: None,
        work: None,
        part: None,
        tokens: Tokens { input: 10, ..Tokens::default() },
        rounds: 1,
        ms: 100,
        usd_micro: Some(5),
        price: None,
        billing: "key".into(),
        outcome: "ok".into(),
        check: None,
    };
    for (age, model) in [(1, "fresh"), (100, "old"), (400, "older")] {
        gateway.app.router.deals.write(&old(age, model)).unwrap();
    }
    assert_eq!(deals(&gateway, &admin, "").await.len(), 3);
    assert_eq!(gateway.app.router.deals.prune(now).unwrap(), (2, 1));
    let left: Vec<String> =
        deals(&gateway, &admin, "").await.iter().map(|deal| deal["model"].as_str().unwrap().to_string()).collect();
    assert_eq!(left, ["fresh"]);
    let stats = message(gateway.get("/api/v1/router/stats").signed(&admin), StatusCode::OK).await;
    assert_eq!(stats["total"]["deals"], 2, "the deal of 100 days ago is still in its day");
    // By time and seat.
    let from = now - 2 * DAY_MS;
    assert_eq!(deals(&gateway, &admin, &format!("?from={from}&seat=fast")).await.len(), 1);
    assert!(deals(&gateway, &admin, "?seat=smart").await.is_empty());
    assert_eq!(status(gateway.get("/api/v1/router/deals?from=вчера").signed(&admin)).await, StatusCode::BAD_REQUEST);

    // A member of the household reads no deals and writes no prices; nobody signed out at all.
    message(
        gateway
            .post("/api/v1/users")
            .signed(&admin)
            .json(&json!({"name": "Аня", "password": "anya-password", "role": "member"})),
        StatusCode::CREATED,
    )
    .await;
    let anya = message(
        gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"})),
        StatusCode::OK,
    )
    .await;
    let member = anya["token"].as_str().unwrap();
    for path in ["/api/v1/router/deals", "/api/v1/router/stats"] {
        assert_eq!(status(gateway.get(path).signed(member)).await, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(status(gateway.get(path)).await, StatusCode::UNAUTHORIZED, "{path}");
    }
    let price = gateway.put("/api/v1/router/prices/env/x").signed(member).json(&json!({"input": 1, "output": 1}));
    assert_eq!(status(price).await, StatusCode::FORBIDDEN);
    assert_eq!(status(gateway.get("/api/v1/router/prices/env/x").signed(member)).await, StatusCode::FORBIDDEN);
    gateway.stop().await;
}

/// The station's answers: the model by the rule of the phrase, and the rule as the deal's reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stations_answers_carry_the_reason_of_the_rule_that_chose_their_model() {
    let llm = ChatStub::start().await;
    let yandex = GatewayYandex::start().await;
    let yandex_url = yandex.url();
    let gateway = Gateway::start(&[
        ("UNIPROXY_MODE", "gateway"),
        ("DIALOG_PROVIDER", "openai"),
        ("LLM_BASE_URL", &llm.url),
        ("LLM_MODEL", "test-llm"),
        ("DIALOG_HOLD_RESPONSE_MS", "300"),
        ("YANDEX_UNIPROXY_URL", &yandex_url),
        ("YANDEX_TIMEOUT_MS", "1500"),
        ("STATION_PAIRING", "open"),
    ])
    .await;
    let admin = setup(&gateway).await;
    let headers: &[(&str, &str)] =
        &[("user-agent", "WebSocket++/0.8.2"), ("x-uprx-oauth-token", "y0_test-secret"), ("x-uprx-uid", "123456")];
    let station = StationClient::connect(&format!("ws://127.0.0.1:{}/uni.ws", gateway.port), headers)
        .await
        .expect("the station connects");
    station.synchronize("sync-router").await;
    llm.set_reply("Потому что так рассеивается свет.");
    let mut listed = Vec::new();
    for (n, (id, text)) in [("hard", "Почему небо голубое?"), ("short", "Как дела?")].into_iter().enumerate()
    {
        station.text_input(&format!("text-{id}"), &format!("req-{id}"), json!({ "type": "text_input", "text": text }));
        let request_id = format!("req-{id}");
        let response =
            station.wait_for(move |m| at(m, "event.aliceResponse.header.requestId") == request_id.as_str(), 8000).await;
        assert!(at(&response, "event.aliceResponse").is_object());
        // The answer's deal before the next phrase: a new one would stop this answer.
        for _ in 0..80 {
            listed = deals(&gateway, &admin, "").await;
            if listed.len() > n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    let reasons: Vec<(&str, &str)> =
        listed.iter().rev().map(|deal| (deal["seat"].as_str().unwrap(), deal["reason"].as_str().unwrap())).collect();
    assert_eq!(reasons, [("smart", "rule:hard"), ("fast", "rule:short")], "{listed:?}");
    station.close();
    gateway.stop().await;
}
