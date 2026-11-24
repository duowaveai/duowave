//! The API's envelope, as the Node gateway's interceptor and filter wrote it, so the panel reads the Rust gateway
//! without a change: a success is `{message, code: 200}` (POST answers 201 unless the route says otherwise, as Nest
//! did), an error is `{code, timestamp, path, message, errors?}`. Routes whose clients expect their own exact format
//! (the station's HTTP checks, its player's files, the call page, the station's scripts) answer without it.

use axum::body::{Body as AxumBody, Bytes};
use axum::extract::{FromRequest, Request};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::utils::time::now_iso;

/// A success in the envelope.
pub struct Api<T>(pub StatusCode, pub T);

/// 200 with the value.
pub fn ok<T: Serialize>(value: T) -> Api<T> {
    Api(StatusCode::OK, value)
}

/// 201 with the value: what Nest answered a POST by default.
pub fn created<T: Serialize>(value: T) -> Api<T> {
    Api(StatusCode::CREATED, value)
}

impl<T: Serialize> IntoResponse for Api<T> {
    fn into_response(self) -> Response {
        match serde_json::to_value(&self.1) {
            Ok(message) => json_response(self.0, &json!({ "message": message, "code": 200 })),
            Err(error) => ApiError::internal(format!("response: {error}")).into_response(),
        }
    }
}

pub type ApiResult<T> = Result<Api<T>, ApiError>;

/// An error as the API answers it. Built by handlers and services alike (Nest's services threw HttpExceptions).
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// Validation errors by field: `{name: ["…"]}`.
    pub errors: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError { status, message: message.into(), errors: None }
    }

    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, message)
    }

    pub fn too_many(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    pub fn unavailable(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    pub fn bad_gateway(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_GATEWAY, message)
    }

    /// 500. The detail goes to the log, not to the client.
    pub fn internal(detail: impl std::fmt::Display) -> ApiError {
        tracing::error!("internal error: {detail}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    }

    /// 400 with the fields' problems, as class-validator listed them.
    pub fn validation(errors: Value) -> ApiError {
        ApiError { status: StatusCode::BAD_REQUEST, message: "Validation Exception".into(), errors: Some(errors) }
    }

    /// 400 for one field.
    pub fn field(field: &str, problem: impl Into<String>) -> ApiError {
        ApiError::validation(json!({ field: [problem.into()] }))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> ApiError {
        ApiError::internal(format!("database: {error}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> ApiError {
        ApiError::internal(format!("json: {error}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> ApiError {
        ApiError::internal(format!("io: {error}"))
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> ApiError {
        match error.downcast::<ApiError>() {
            Ok(api) => api,
            Err(error) => ApiError::internal(format!("{error:#}")),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // The body needs the request's path: `error_bodies` fills it in on the way out.
        let mut response = self.status.into_response();
        response.extensions_mut().insert(self);
        response
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut response = Response::new(AxumBody::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    response
}

/// Middleware: an ApiError gets its body with the request's path (and query, as Express's `request.url` had it).
pub async fn error_bodies(request: Request, next: Next) -> Response {
    let path = request.uri().path_and_query().map(|pq| pq.as_str().to_string()).unwrap_or_else(|| "/".into());
    let mut response = next.run(request).await;
    if let Some(error) = response.extensions_mut().remove::<ApiError>() {
        let mut body = json!({
            "code": error.status.as_u16(),
            "timestamp": now_iso(),
            "path": path,
            "message": error.message,
        });
        if let Some(errors) = error.errors {
            body["errors"] = errors;
        }
        let mut replaced = json_response(error.status, &body);
        for (name, value) in response.headers() {
            if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
                replaced.headers_mut().insert(name.clone(), value.clone());
            }
        }
        return replaced;
    }
    response
}

/// What no route answers: Express's «Cannot GET /x» as a 404 (a wrong method too).
pub async fn not_found(method: Method, request: Request) -> ApiError {
    ApiError::not_found(format!("Cannot {} {}", method, request.uri().path()))
}

/// A JSON body: empty is `{}` (Nest gave handlers `{}` for no body), anything else must parse; a field that does
/// not fit answers 400 with its path.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|error| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, error.to_string()))?;
        let text: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"{}" } else { &bytes };
        let mut deserializer = serde_json::Deserializer::from_slice(text);
        match serde_path_to_error::deserialize(&mut deserializer) {
            Ok(value) => Ok(Body(value)),
            Err(error) => {
                let path = error.path().to_string();
                let inner = error.into_inner();
                if inner.is_syntax() || inner.is_eof() {
                    Err(ApiError::bad_request(format!("Unexpected JSON: {inner}")))
                } else {
                    let field = if path == "." { "body".to_string() } else { path };
                    Err(ApiError::field(&field, inner.to_string()))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(router: Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let request =
            axum::http::Request::builder().method(method).uri(uri).body(AxumBody::from(body.to_string())).unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[derive(serde::Deserialize, Serialize)]
    struct Named {
        name: String,
    }

    fn router() -> Router {
        Router::new()
            .route("/ok", get(|| async { ok(json!({"a": 1})) }))
            .route("/none", get(|| async { ok(()) }))
            .route("/named", post(|Body(named): Body<Named>| async move { created(named) }))
            .route("/fail", get(|| async { Err::<Api<()>, _>(ApiError::forbidden("Нет права.")) }))
            .fallback(not_found)
            .layer(axum::middleware::from_fn(error_bodies))
    }

    #[tokio::test]
    async fn wraps_successes_and_errors_as_the_panel_reads_them() {
        assert_eq!(call(router(), "GET", "/ok", "").await, (StatusCode::OK, json!({"message": {"a": 1}, "code": 200})));
        assert_eq!(call(router(), "GET", "/none", "").await, (StatusCode::OK, json!({"message": null, "code": 200})));
        let (status, body) = call(router(), "GET", "/fail?x=1", "").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], 403);
        assert_eq!(body["path"], "/fail?x=1");
        assert_eq!(body["message"], "Нет права.");
        assert!(body.get("errors").is_none());
        assert!(body["timestamp"].as_str().unwrap().ends_with('Z'));
        let (status, body) = call(router(), "DELETE", "/nowhere", "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["message"], "Cannot DELETE /nowhere");
    }

    #[tokio::test]
    async fn reads_bodies_and_names_the_field_that_does_not_fit() {
        assert_eq!(call(router(), "POST", "/named", r#"{"name":"Аня"}"#).await.0, StatusCode::CREATED);
        let (status, body) = call(router(), "POST", "/named", r#"{"name":5}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], "Validation Exception");
        assert!(body["errors"]["name"][0].is_string(), "{body}");
        let (status, body) = call(router(), "POST", "/named", "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "an empty body is {{}}, and {{}} has no name: {body}");
        assert_eq!(call(router(), "POST", "/named", "{oops").await.0, StatusCode::BAD_REQUEST);
    }
}
