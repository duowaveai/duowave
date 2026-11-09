//! The panel's live channel end to end, as the panel reads it: a signed GET, Server-Sent Events as they come; the
//! journal of the same requests; the streams' end when the gateway stops. (The Node test drove requests through a
//! station; here the RequestStarter the sessions will use starts them.)

mod support;

use std::sync::{Arc, Mutex};

use duowave::core::requests::{Place, RequestKind, RequestPhase, RequestStart, RequestStarter, RequestStatus};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, until, Gateway};

struct Live {
    messages: Arc<Mutex<Vec<Value>>>,
    ended: Arc<Mutex<bool>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Live {
    async fn open(gateway: &Gateway, token: &str) -> Live {
        let (name, value) = bearer(token);
        let response = gateway.get("/api/v1/live").header(name, value).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let kind = response.headers()["content-type"].to_str().unwrap().to_string();
        assert!(kind.starts_with("text/event-stream"), "{kind}");
        assert_eq!(response.headers()["cache-control"], "no-cache, no-transform");
        let messages = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(Mutex::new(false));
        let (into, done) = (messages.clone(), ended.clone());
        let reader = tokio::spawn(async move {
            let mut response = response;
            let mut buffer = String::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n") {
                    let block: String = buffer.drain(..end + 2).collect();
                    let data: Vec<&str> = block.lines().filter_map(|line| line.strip_prefix("data: ")).collect();
                    if !data.is_empty() {
                        into.lock().unwrap().push(serde_json::from_str(&data.join("\n")).unwrap());
                    }
                }
            }
            *done.lock().unwrap() = true;
        });
        Live { messages, ended, reader }
    }

    fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages.lock().unwrap().iter().filter(|message| message["type"] == kind).cloned().collect()
    }

    fn requests(&self) -> Vec<Value> {
        self.of_type("request").into_iter().map(|message| message["request"].clone()).collect()
    }
}

async fn setup(gateway: &Gateway) -> String {
    let setup =
        message(gateway.post("/api/v1/auth/setup").json(&json!({"password": "admin-password-1"})), StatusCode::CREATED);
    setup.await["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn follows_requests_live_on_the_panels_channel_the_trip_the_words_the_end_and_keeps_them_in_the_journal() {
    let gateway = Gateway::start(&[("UNIPROXY_MODE", "gateway")]).await;
    let admin = setup(&gateway).await;
    let member = json!({"name": "Аня", "password": "anya-password", "role": "member"});
    let (name, value) = bearer(&admin);
    message(gateway.post("/api/v1/users").header(name, value).json(&member), StatusCode::CREATED).await;
    let anya = message(
        gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"})),
        StatusCode::OK,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let reader = Live::open(&gateway, &admin).await;
    let viewer = Live::open(&gateway, &anya).await;
    assert!(until(2000, || !reader.of_type("hello").is_empty() && !viewer.of_type("hello").is_empty()).await);
    let hello = reader.of_type("hello").remove(0);
    assert!(hello["presence"].is_array());
    assert!(chrono::DateTime::parse_from_rfc3339(hello["serverTime"].as_str().unwrap()).is_ok());

    let requests: &dyn RequestStarter = gateway.app.requests.as_ref();
    let trip = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: Some("s1".into()),
        kind: RequestKind::Text,
        text: Some("Который час?".into()),
        parent: None,
    });
    tokio::task::yield_now().await;
    trip.routed(Some("chat"), "own");
    trip.step(RequestPhase::Think, Some(Place::Own), Some("Разговор, время, дата"));
    trip.step(RequestPhase::Speak, Some(Place::Yandex), Some("Алиса"));
    trip.answered("12:00.");
    trip.finish(RequestStatus::Answered, None, None);

    let ended = |live: &Live| live.requests().iter().any(|request| request["status"] == "answered");
    assert!(until(2000, || ended(&reader) && ended(&viewer)).await, "{:?}", reader.messages.lock().unwrap());
    let updates = reader.requests();
    assert_eq!((updates[0]["status"].as_str(), updates[0]["mode"].as_str()), (Some("active"), Some("gateway")));
    let done = updates.last().unwrap();
    assert_eq!((done["capability"].as_str(), done["executor"].as_str()), (Some("chat"), Some("own")));
    assert_eq!((done["text"].as_str(), done["answer"].as_str()), (Some("Который час?"), Some("12:00.")));
    let phases: Vec<&str> =
        done["steps"].as_array().unwrap().iter().filter_map(|step| step["phase"].as_str()).collect();
    assert_eq!(phases, ["think", "speak"]);
    assert_eq!(done["steps"][1]["by"], "yandex");
    // Who may not read the journal sees the trip, not the words.
    let bare = viewer.requests().last().unwrap().clone();
    assert_eq!((bare["status"].as_str(), bare.get("text"), bare.get("answer")), (Some("answered"), None, None));
    assert_eq!(bare["steps"], done["steps"]);

    let (name, value) = bearer(&admin);
    let journal = message(gateway.get("/api/v1/requests?limit=10").header(name, value), StatusCode::OK).await;
    assert_eq!(journal["keepDays"], 30);
    let row = journal["requests"].as_array().unwrap().iter().find(|row| row["id"] == done["id"]).unwrap().clone();
    assert_eq!((row["text"].as_str(), row["status"].as_str()), (Some("Который час?"), Some("answered")));
    let (name, value) = bearer(&anya);
    assert_eq!(
        message(gateway.get("/api/v1/requests").header(name, value), StatusCode::FORBIDDEN).await,
        "Нет права: читать журнал запросов: что говорили колонке.",
        "the journal is for journal.view"
    );

    // How long the journal is kept is a setting of the house.
    let (name, value) = bearer(&admin);
    let kept =
        message(gateway.put("/api/v1/requests/keep").header(name, value).json(&json!({"days": 7})), StatusCode::OK);
    assert_eq!(kept.await, json!({"days": 7}));
    let changed = || reader.of_type("changed").iter().any(|message| message["name"] == "settings.changed");
    assert!(until(2000, changed).await, "the panel re-reads what changed elsewhere");
}

#[tokio::test]
async fn ends_the_open_streams_when_the_gateway_stops_and_journals_what_was_going() {
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    let live = Live::open(&gateway, &admin).await;
    assert!(until(2000, || !live.of_type("hello").is_empty()).await);
    let requests: &dyn RequestStarter = gateway.app.requests.as_ref();
    let going = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: None,
        kind: RequestKind::Voice,
        text: None,
        parent: None,
    });
    let app = gateway.app.clone();
    let ended = live.ended.clone();
    tokio::time::timeout(std::time::Duration::from_secs(5), gateway.stop()).await.expect("the server stops at once");
    assert!(until(2000, || *ended.lock().unwrap()).await, "the stream ended");
    live.reader.abort();
    assert_eq!(going.snapshot().status, RequestStatus::Cancelled);
    let journal = app.requests.history(&Default::default()).unwrap();
    assert_eq!(journal[0].error.as_deref(), Some("the gateway stopped"));
}
