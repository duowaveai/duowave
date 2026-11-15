//! The program's HTTP on `127.0.0.1:$PORT`, by the app protocol (docs/plugins/AUTHORING.md, «Приложения»):
//! `GET /health` without a token; `POST /call/<function>` with `Authorization: Bearer $DUOWAVE_APP_TOKEN` and JSON in
//! and out — an error is `{ "error": "words in the household's language" }` with 400, an unknown function 404.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::app::App;
use crate::i18n::t;

const BODY_LIMIT: usize = 256 * 1024;

fn answer(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap_or_default())))
        .expect("a plain response")
}

async fn handle(
    app: Arc<App>,
    token: Arc<String>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = request.uri().path().to_string();
    if path == "/health" {
        return Ok(answer(StatusCode::OK, &json!({ "ok": true, "engine": app.engine.running() })));
    }
    let Some(function) = path.strip_prefix("/call/").map(str::to_string) else {
        return Ok(answer(StatusCode::NOT_FOUND, &json!({ "error": "not found" })));
    };
    let authorized = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| !token.is_empty() && given == token.as_str());
    if !authorized {
        return Ok(answer(StatusCode::UNAUTHORIZED, &json!({ "error": "unauthorized" })));
    }
    let body = match Limited::new(request.into_body(), BODY_LIMIT).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Ok(answer(StatusCode::PAYLOAD_TOO_LARGE, &json!({ "error": "too large" }))),
    };
    let input: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).unwrap_or(json!({})) };
    match app.call(&function, &input).await {
        Ok(value) => Ok(answer(StatusCode::OK, &value)),
        Err((key, _)) if key == "not_found" => {
            Ok(answer(StatusCode::NOT_FOUND, &json!({ "error": "no such function" })))
        }
        Err((key, detail)) => {
            let locale = app.gateway.locale().await;
            Ok(answer(StatusCode::BAD_REQUEST, &json!({ "error": t(&locale, &key, &detail) })))
        }
    }
}

pub async fn serve(app: Arc<App>, port: u16, token: String) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let token = Arc::new(token);
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("own-server: accept: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let app = app.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(app.clone(), token.clone(), request));
            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
        });
    }
}
