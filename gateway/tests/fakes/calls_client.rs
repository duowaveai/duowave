//! A client of our calls WebSocket: the call page or the station simulator. Keeps its events.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::{lock, sleep};

pub struct CallsClient {
    pub events: Arc<Mutex<Vec<Value>>>,
    out: mpsc::UnboundedSender<Message>,
}

impl CallsClient {
    /// Connects, says hello and waits for the welcome.
    pub async fn connect(url: &str, hello: Value) -> CallsClient {
        let (socket, _) = tokio_tungstenite::connect_async(url).await.expect("the calls socket opens");
        let (mut sink, mut incoming) = socket.split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let into = events.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = incoming.next().await {
                if let Message::Text(text) = message {
                    if let Ok(event) = serde_json::from_str::<Value>(text.as_str()) {
                        lock(&into).push(event);
                    }
                }
            }
        });
        let client = CallsClient { events, out };
        client.send(hello);
        client.wait_for(|event| event["type"] == "welcome", 4000).await;
        client
    }

    pub fn send(&self, message: Value) {
        let _ = self.out.send(Message::Text(message.to_string().into()));
    }

    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    pub async fn wait_for(&self, predicate: impl Fn(&Value) -> bool, timeout_ms: u64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if let Some(found) = lock(&self.events).iter().find(|event| predicate(event)).cloned() {
                return found;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out; events: {}",
                Value::Array(lock(&self.events).clone())
            );
            sleep(20).await;
        }
    }
}
