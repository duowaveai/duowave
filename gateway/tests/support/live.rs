//! The panel's live channel as the panel reads it — its stream (`/api/v1/live`, Server-Sent Events) and its socket
//! (`/api/v1/live/ws` with a ticket) — and the first admin: the live channel's tests share them (tests/live.rs, tests/live_computer.rs).

use std::sync::{Arc, Mutex};

use reqwest::StatusCode;
use serde_json::{json, Value};

use super::{bearer, message, Gateway};

pub struct Live {
    pub messages: Arc<Mutex<Vec<Value>>>,
    pub ended: Arc<Mutex<bool>>,
    pub reader: tokio::task::JoinHandle<()>,
}

impl Live {
    pub async fn open(gateway: &Gateway, token: &str) -> Live {
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

    pub fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages.lock().unwrap().iter().filter(|message| message["type"] == kind).cloned().collect()
    }

    pub fn requests(&self) -> Vec<Value> {
        self.of_type("request").into_iter().map(|message| message["request"].clone()).collect()
    }
}

pub struct Socket {
    pub messages: Arc<Mutex<Vec<Value>>>,
    /// The close frame's code, once it came.
    pub closed: Arc<Mutex<Option<u16>>>,
    pub reader: tokio::task::JoinHandle<()>,
}

impl Socket {
    pub async fn ticket(gateway: &Gateway, token: &str) -> String {
        let (name, value) = bearer(token);
        let ticket = message(gateway.post("/api/v1/live/ticket").header(name, value), StatusCode::CREATED).await;
        assert_eq!(ticket["expiresIn"], 30);
        ticket["ticket"].as_str().unwrap().to_string()
    }

    pub fn address(gateway: &Gateway, ticket: &str) -> String {
        format!("ws://127.0.0.1:{}/api/v1/live/ws?ticket={ticket}", gateway.port)
    }

    pub async fn open(gateway: &Gateway, token: &str) -> Socket {
        let ticket = Socket::ticket(gateway, token).await;
        let (client, _) = tokio_tungstenite::connect_async(Socket::address(gateway, &ticket)).await.unwrap();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(Mutex::new(None));
        let (into, end) = (messages.clone(), closed.clone());
        let reader = tokio::spawn(async move {
            use futures::StreamExt;
            use tokio_tungstenite::tungstenite::Message;
            let mut client = client;
            while let Some(Ok(frame)) = client.next().await {
                match frame {
                    Message::Text(text) => into.lock().unwrap().push(serde_json::from_str(text.as_str()).unwrap()),
                    Message::Close(frame) => {
                        *end.lock().unwrap() = Some(frame.map(|frame| u16::from(frame.code)).unwrap_or(1005));
                    }
                    _ => {}
                }
            }
        });
        Socket { messages, closed, reader }
    }

    pub fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages.lock().unwrap().iter().filter(|message| message["type"] == kind).cloned().collect()
    }

    /// What a station said, relayed: `None` — its channel lost. Only a computer relays it.
    pub fn station(&self, key: &str) -> Vec<Option<Value>> {
        self.of_type("station")
            .into_iter()
            .filter(|message| message["station"] == key)
            .map(|message| message.get("message").cloned())
            .collect()
    }
}

/// The first admin's token.
pub async fn setup(gateway: &Gateway) -> String {
    let setup =
        message(gateway.post("/api/v1/auth/setup").json(&json!({"password": "admin-password-1"})), StatusCode::CREATED);
    setup.await["token"].as_str().unwrap().to_string()
}
