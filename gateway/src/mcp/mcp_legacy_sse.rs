//! The transport of MCP servers from before Streamable HTTP (2024-11-05): a GET opens an event stream whose first
//! event, `endpoint`, says where to POST; every message the server sends comes on that stream. rmcp no longer has
//! it, so it is here, as an rmcp transport: the session above it is the SDK's.

use std::future::Future;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use futures::StreamExt;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::RoleClient;
use sse_stream::SseStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("the server refused the token (401)")]
    Unauthorized,
    #[error("HTTP {0}")]
    Status(StatusCode),
    #[error("not an event stream")]
    NotEventStream,
    #[error("the stream closed before the server named its endpoint")]
    NoEndpoint,
    #[error("endpoint origin does not match connection origin: {0}")]
    ForeignEndpoint(String),
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Url(#[from] url::ParseError),
}

pub struct LegacySseTransport {
    client: reqwest::Client,
    endpoint: Url,
    headers: HeaderMap,
    incoming: mpsc::Receiver<ServerJsonRpcMessage>,
    stop: CancellationToken,
}

impl LegacySseTransport {
    /// Opens the stream and waits for the endpoint; `headers` go with every request (a token among them).
    pub async fn connect(
        client: reqwest::Client,
        url: &str,
        headers: HeaderMap,
    ) -> Result<LegacySseTransport, SseError> {
        let base = Url::parse(url)?;
        let response = client
            .get(base.clone())
            .headers(headers.clone())
            .header(header::ACCEPT, HeaderValue::from_static("text/event-stream"))
            .send()
            .await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(SseError::Unauthorized);
        }
        if !status.is_success() {
            return Err(SseError::Status(status));
        }
        let event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !event_stream {
            return Err(SseError::NotEventStream);
        }
        let mut events = SseStream::from_bytes_stream(response.bytes_stream());
        let endpoint = loop {
            match events.next().await {
                Some(Ok(event)) if event.event.as_deref() == Some("endpoint") => {
                    break event.data.unwrap_or_default();
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return Err(SseError::NoEndpoint),
            }
        };
        let endpoint = base.join(endpoint.trim())?;
        if endpoint.origin() != base.origin() {
            return Err(SseError::ForeignEndpoint(endpoint.to_string()));
        }
        let (sender, incoming) = mpsc::channel(64);
        let stop = CancellationToken::new();
        let reading = stop.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = reading.cancelled() => break,
                    event = events.next() => event,
                };
                match event {
                    Some(Ok(event)) if matches!(event.event.as_deref(), None | Some("message")) => {
                        let Some(data) = event.data else { continue };
                        match serde_json::from_str::<ServerJsonRpcMessage>(&data) {
                            Ok(message) => {
                                if sender.send(message).await.is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                tracing::debug!(target: "mcp", "an SSE message that is not JSON-RPC: {error}")
                            }
                        }
                    }
                    Some(Ok(_)) => continue,
                    // The stream ended or broke: the session sees the transport closed.
                    Some(Err(_)) | None => break,
                }
            }
        });
        Ok(LegacySseTransport { client, endpoint, headers, incoming, stop })
    }
}

impl Transport<RoleClient> for LegacySseTransport {
    type Error = SseError;

    fn send(&mut self, item: ClientJsonRpcMessage) -> impl Future<Output = Result<(), SseError>> + Send + 'static {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let headers = self.headers.clone();
        async move {
            let response = client.post(endpoint).headers(headers).json(&item).send().await?;
            match response.status() {
                StatusCode::UNAUTHORIZED => Err(SseError::Unauthorized),
                status if status.is_success() => Ok(()),
                status => Err(SseError::Status(status)),
            }
        }
    }

    fn receive(&mut self) -> impl Future<Output = Option<ServerJsonRpcMessage>> + Send {
        self.incoming.recv()
    }

    async fn close(&mut self) -> Result<(), SseError> {
        self.stop.cancel();
        self.incoming.close();
        Ok(())
    }
}

impl Drop for LegacySseTransport {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
