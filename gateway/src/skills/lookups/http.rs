//! A GET (and a POST for the smart home) for lookups: JSON or text, a timeout, readable errors. Behind traits, so
//! the skills are checked against local fakes.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::utils::http::client;

/// A lookup's answer: its status and its body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpText {
    pub status: u16,
    pub text: String,
}

pub trait HttpGet: Send + Sync {
    fn get(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<HttpText>>;
}

pub trait HttpPost: Send + Sync {
    /// The status of a JSON POST.
    fn post(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<u16>>;
}

/// A lookup gives up after this long, or when the request is cancelled.
const GET_TIMEOUT: Duration = Duration::from_secs(8);

/// The gateway's own client (rustls with built-in roots).
#[derive(Debug, Clone, Copy, Default)]
pub struct Http;

impl HttpGet for Http {
    fn get(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<HttpText>> {
        Box::pin(async move {
            let mut request = client().get(&url).timeout(GET_TIMEOUT);
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            let answer = async {
                let response = request.send().await?;
                let status = response.status().as_u16();
                anyhow::Ok(HttpText { status, text: response.text().await? })
            };
            tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("the request was cancelled"),
                result = answer => result,
            }
        })
    }
}

impl HttpPost for Http {
    fn post(
        &self,
        url: String,
        body: Value,
        headers: Vec<(String, String)>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<u16>> {
        Box::pin(async move {
            let mut request = client().post(&url).body(body.to_string());
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("the request was cancelled"),
                response = request.send() => Ok(response?.status().as_u16()),
            }
        })
    }
}

/// The host of a URL with its port, as URL.host writes it.
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_string();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_default()
}

/// A GET whose answer must be 2xx JSON.
pub async fn get_json(
    get: &dyn HttpGet,
    url: &str,
    cancel: &CancellationToken,
    headers: Vec<(String, String)>,
) -> anyhow::Result<Value> {
    let HttpText { status, text } = get.get(url.to_string(), headers, cancel.clone()).await?;
    if !(200..300).contains(&status) {
        let start: String = text.chars().take(120).collect();
        anyhow::bail!("HTTP {status} from {}: {start}", host_of(url));
    }
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Router;

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn gets_json_and_says_who_failed() {
        let base = serve(
            Router::new()
                .route("/ok", get(|| async { r#"{"results": [1]}"# }))
                .route("/down", get(|| async { (axum::http::StatusCode::BAD_GATEWAY, "upstream is down") }))
                .route(
                    "/hook",
                    post(|headers: axum::http::HeaderMap, body: String| async move {
                        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                        if auth == "Bearer t" && body.contains("light.kitchen") {
                            axum::http::StatusCode::OK
                        } else {
                            axum::http::StatusCode::UNAUTHORIZED
                        }
                    }),
                ),
        )
        .await;
        let cancel = CancellationToken::new();
        let value = get_json(&Http, &format!("{base}/ok"), &cancel, Vec::new()).await.unwrap();
        assert_eq!(value, serde_json::json!({"results": [1]}));
        let error = get_json(&Http, &format!("{base}/down"), &cancel, Vec::new()).await.unwrap_err().to_string();
        assert_eq!(error, format!("HTTP 502 from {}: upstream is down", base.trim_start_matches("http://")));
        let headers = vec![("Authorization".to_string(), "Bearer t".to_string())];
        let status = Http.post(
            format!("{base}/hook"),
            serde_json::json!({"entity_id": "light.kitchen"}),
            headers,
            cancel.clone(),
        );
        assert_eq!(status.await.unwrap(), 200);
        cancel.cancel();
        assert!(Http.get(format!("{base}/ok"), Vec::new(), cancel).await.is_err());
    }
}
