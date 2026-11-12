//! Signing in to an outside service instead of pasting its key: the owner's account at the service gives the gateway
//! its access. One engine for every service whose flow is here: it begins a sign-in (a PKCE verifier and an id that
//! is also the OAuth state), sends the owner to the service's page and takes the return — the browser back on this
//! computer (the panel is open here), or, from another device (a phone, a computer looking at a station's gateway),
//! the code the page shows or the address the browser could not open, pasted into the panel. The code is exchanged
//! for what the service gives; who began the sign-in (a model provider of «Мозг») keeps it.
//!
//! Two flows. `KeyExchange` (OpenRouter) brings a key that lives until the owner deletes it, kept as a pasted one.
//! `Tokens` (ChatGPT's plan) brings an access token for about an hour and a refresh token: tokens.rs keeps them on this
//! gateway and renews them before each request that needs it.
//!
//! A service names its flow in its preset (src/dialog/brain/presets.rs): a service whose flow is already here is one
//! more preset, no code. MCP servers sign in their own way (src/mcp/mcp_oauth.rs: discovery and registration by the
//! MCP spec, with their return address registered at the service) and share the return page (page.rs). The plan and
//! the services' terms — docs/plans/provider-sign-in.md.

pub mod controller;
pub mod page;
pub mod tokens;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures::StreamExt;
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub use controller::routes;
pub use tokens::{AccountView, Problem, TokenAccess, TokenFlow, TokenRecord};

use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::utils::http::client_builder;
use crate::utils::ids::{random_bytes, random_hex, uuid};

/// Where the browser comes back on this computer.
pub const CALLBACK_PATH: &str = "/api/v1/oauth/callback";
/// A sign-in waits this long for its return; the services' codes live less (OpenRouter's ten minutes).
const WAIT: Duration = Duration::from_secs(15 * 60);
/// An exchange's or a renewal's longest wait.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest answer of an exchange read.
const BODY_LIMIT: usize = 64 * 1024;
/// Our name at a service: the label of a key a sign-in creates, the name at a first registration.
const APP_NAME: &str = "DuoWave";
/// This gateway's stable id for the services that want one (`ext_agent_host_id`): its own, not among the secrets that
/// travel with the settings to another computer.
const HOST_ID: &str = "oauth:host";

/// How a service lets the owner sign in instead of pasting a key: the name, the button's words when the service sets
/// them («Continue with ChatGPT»), and the flow. The panel learns the name, the words, and whether a key is an
/// alternative at all (`keyless`: the access is the account's, as with ChatGPT's plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignIn {
    /// The button says «Войти через {with}» unless `label` says otherwise.
    pub with: &'static str,
    pub label: Option<&'static str>,
    pub flow: Flow,
}

impl Serialize for SignIn {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("SignIn", 3)?;
        out.serialize_field("with", self.with)?;
        match self.label {
            Some(label) => out.serialize_field("label", label)?,
            None => out.skip_field("label")?,
        }
        match self.flow {
            Flow::Tokens(_) => out.serialize_field("keyless", &true)?,
            Flow::KeyExchange { .. } => out.skip_field("keyless")?,
        }
        out.end()
    }
}

/// The flows the gateway knows. A new service with one of them is data in its preset; a new flow is code here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// A code exchanged for a long-lived key of the owner's, as OpenRouter's PKCE does it
    /// (https://openrouter.ai/docs/use-cases/oauth-pkce): `authorize` takes `callback_url` and `state` — or neither,
    /// and the page shows the code — with `code_challenge`; `exchange` takes the code with the verifier and answers
    /// `{"key": …}`. Nothing to renew: the key lives until the owner deletes it in the account.
    KeyExchange { authorize: &'static str, exchange: &'static str },
    /// OAuth 2.0 with PKCE and tokens kept and renewed (tokens.rs): ChatGPT's plan.
    Tokens(&'static TokenFlow),
}

/// Where the owner's browser brings the return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Return {
    /// To this computer by itself: the panel is open on the gateway's own computer.
    Here,
    /// The service's page shows a code, pasted into the panel.
    Code,
    /// The browser lands on an address of its own device that does not open (the service returns only to
    /// 127.0.0.1); that address is pasted into the panel.
    Address,
}

/// Who began a sign-in and gets what it brings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A model provider of «Мозг»: the one created at `created_at`, not a later one that took its id.
    Provider { id: String, created_at: String },
}

impl Target {
    /// Where its tokens are kept.
    pub fn record_id(&self) -> String {
        match self {
            Target::Provider { id, .. } => record_id(id),
        }
    }
}

/// A provider's tokens: on this gateway only (`oauth:` does not travel with the settings, src/placement/transfer.rs).
pub fn record_id(provider: &str) -> String {
    format!("oauth:llm:{provider}")
}

/// What a sign-in brought, for whoever began it to keep among its secrets. No Debug: it would print the value.
pub enum Credential {
    /// A key, kept as a pasted one would be.
    Key(String),
    /// Tokens, kept on this gateway and renewed (tokens.rs).
    Tokens(TokenRecord),
}

/// What a return brought: the code, and the client a service names when it registered one on the fly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Given {
    pub code: String,
    pub client_id: Option<String>,
}

/// A sign-in begun, for the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInStart {
    /// The panel asks the sign-in's state by it and sends it with a pasted code.
    pub id: String,
    /// The service's page for the owner.
    pub url: String,
    pub mode: Return,
}

/// Where a sign-in is, for the panel that waits for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SignInStatus {
    /// The owner has not come back yet.
    Waiting,
    /// The code is being exchanged.
    Working,
    /// Signed in: the title of who began it («OpenRouter»).
    Done { title: String },
    /// It did not work: why, in words.
    Failed { message: String },
}

struct Entry {
    target: Target,
    with: &'static str,
    flow: Flow,
    verifier: String,
    nonce: String,
    redirect: String,
    client: String,
    at: Instant,
    status: SignInStatus,
}

/// A sign-in taken for its exchange: no other return of it is taken meanwhile.
pub struct Taken {
    pub id: String,
    pub target: Target,
    pub with: &'static str,
    flow: Flow,
    verifier: String,
    nonce: String,
    redirect: String,
    client: String,
}

pub struct OAuthService {
    port: u16,
    callback: String,
    secrets: Arc<SecretsService>,
    /// The sign-ins begun, by id; one older than `WAIT` is dropped.
    entries: Mutex<HashMap<String, Entry>>,
}

/// An address on this computer for a service's return: `localhost`, with the port unless it is 80.
pub fn return_url(port: u16, path: &str) -> String {
    let host = if port == 0 || port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}{path}")
}

/// The same at 127.0.0.1, for a service that takes no other name (ChatGPT).
pub fn loopback_url(port: u16, path: &str) -> String {
    let host = if port == 0 || port == 80 { "127.0.0.1".to_string() } else { format!("127.0.0.1:{port}") };
    format!("http://{host}{path}")
}

/// A PKCE verifier: 32 random bytes, base64url (RFC 7636, 43 characters).
fn verifier() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(32))
}

/// The S256 challenge of a verifier.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What a service may hand over as a key, a token or a code: printable ASCII without spaces (it goes into a header or a
/// form as it is), not empty, not huge.
pub fn plain_token(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|byte| byte.is_ascii_graphic())
}

pub fn stale() -> ApiError {
    ApiError::bad_request("Этот вход устарел или начат не здесь: нажмите «Войти» ещё раз.")
}

/// The return of a sign-in the service did not confirm (its own words are never shown).
fn refused(with: &str) -> ApiError {
    ApiError::bad_request(format!("Войти не получилось: сервис {with} не подтвердил вход. Нажмите «Войти» ещё раз."))
}

/// A request to a service's exchange that did not go through: the status and the OAuth error code when it answered.
#[derive(Debug)]
pub struct CallError {
    pub status: Option<u16>,
    pub code: Option<String>,
    detail: String,
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.status, &self.code) {
            (Some(status), Some(code)) => write!(f, "HTTP {status}: {code}"),
            (Some(status), None) if self.detail.is_empty() => write!(f, "HTTP {status}"),
            (Some(status), None) => write!(f, "HTTP {status}: {}", self.detail),
            (None, _) => write!(f, "cannot reach the service: {}", self.detail),
        }
    }
}

/// The client of the exchanges and renewals: never redirected (the code, the verifier and the tokens go only where the
/// flow says), a short wait.
pub(crate) fn http_client() -> Result<reqwest::Client, CallError> {
    client_builder()
        .timeout(EXCHANGE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| CallError { status: None, code: None, detail: error.to_string() })
}

/// A request whose answer is JSON: the answer, or the status with the OAuth error code (`{"error": "invalid_grant"}`,
/// or `{"error": {"code": …}}`). The service's own text is not kept.
pub(crate) async fn call(request: reqwest::RequestBuilder) -> Result<Value, CallError> {
    let response = request.send().await.map_err(|error| CallError {
        status: None,
        code: None,
        detail: crate::dialog::llm::describe_http_error(&error),
    })?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|error| CallError {
            status: Some(status.as_u16()),
            code: None,
            detail: error.to_string(),
        })?;
        if bytes.len() + chunk.len() > BODY_LIMIT {
            return Err(CallError {
                status: Some(status.as_u16()),
                code: None,
                detail: "the answer is too large".into(),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let code = match body.get("error") {
            Some(Value::String(code)) => Some(code.clone()),
            Some(Value::Object(error)) => {
                error.get("code").or_else(|| error.get("type")).and_then(Value::as_str).map(str::to_string)
            }
            _ => None,
        };
        return Err(CallError { status: Some(status.as_u16()), code, detail: String::new() });
    }
    serde_json::from_slice(&bytes).map_err(|error| CallError {
        status: Some(status.as_u16()),
        code: None,
        detail: format!("the answer is not JSON: {error}"),
    })
}

impl OAuthService {
    /// The engine; `port` is where a browser on this computer reaches the gateway.
    pub fn new(port: u16, secrets: Arc<SecretsService>) -> OAuthService {
        OAuthService { port, callback: return_url(port, CALLBACK_PATH), secrets, entries: Mutex::new(HashMap::new()) }
    }

    pub fn callback_url(&self) -> &str {
        &self.callback
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// This gateway's stable id for the services that want one: made at the first such sign-in and kept.
    fn host_id(&self) -> Result<String, ApiError> {
        let _one = self.lock();
        if let Some(id) = self.secrets.get(HOST_ID) {
            return Ok(id);
        }
        let id = format!("urn:uuid:{}", uuid());
        self.secrets.set(HOST_ID, Some(&id))?;
        Ok(id)
    }

    /// Begins a sign-in: the service's page for the owner, and the id the return finds it by. `here`: the panel is
    /// open on this computer, so the browser's return reaches the gateway.
    pub fn start(&self, sign_in: &SignIn, target: Target, here: bool) -> Result<SignInStart, ApiError> {
        let id = random_hex(16);
        let verifier = verifier();
        let nonce = random_hex(16);
        let (url, mode, redirect, client) = match sign_in.flow {
            Flow::KeyExchange { authorize, .. } => {
                let mut url = url::Url::parse(authorize).expect("a flow's address is a URL");
                {
                    let mut query = url.query_pairs_mut();
                    if here {
                        query.append_pair("callback_url", &self.callback);
                        query.append_pair("state", &id);
                    }
                    query.append_pair("code_challenge", &challenge(&verifier));
                    query.append_pair("code_challenge_method", "S256");
                    query.append_pair("key_label", APP_NAME);
                }
                (url.to_string(), if here { Return::Here } else { Return::Code }, String::new(), String::new())
            }
            Flow::Tokens(flow) => {
                // A later sign-in goes with the client registered at the first one, and says whose it was.
                let earlier = self
                    .secrets
                    .get(&target.record_id())
                    .and_then(|text| serde_json::from_str::<TokenRecord>(&text).ok());
                let client = earlier
                    .as_ref()
                    .map(|record| record.client_id.clone())
                    .filter(|client| !client.is_empty())
                    .unwrap_or_else(|| flow.client.to_string());
                let first = flow.registers && client == flow.client;
                let redirect = match flow.loopback {
                    Some(path) => loopback_url(self.port, path),
                    None => self.callback.clone(),
                };
                let host = if flow.host_id { Some(self.host_id()?) } else { None };
                let mut url = url::Url::parse(flow.authorize).expect("a flow's address is a URL");
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("client_id", &client);
                    if let Some(host) = &host {
                        if first {
                            query.append_pair("agent_name_hint", APP_NAME);
                        }
                        query.append_pair("ext_agent_host_id", host);
                    }
                    if let Some(earlier) = earlier.as_ref().filter(|_| !first) {
                        if !earlier.id_token.is_empty() {
                            query.append_pair("id_token_hint", &earlier.id_token);
                        }
                        if let Some(email) = &earlier.email {
                            query.append_pair("login_hint", email);
                        }
                    }
                    query.append_pair("response_type", "code");
                    query.append_pair("redirect_uri", &redirect);
                    query.append_pair("scope", flow.scope);
                    if let Some(resource) = flow.resource {
                        query.append_pair("resource", resource);
                    }
                    query.append_pair("state", &id);
                    query.append_pair("nonce", &nonce);
                    query.append_pair("code_challenge", &challenge(&verifier));
                    query.append_pair("code_challenge_method", "S256");
                }
                (url.to_string(), if here { Return::Here } else { Return::Address }, redirect, client)
            }
        };
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.at.elapsed() <= WAIT);
        entries.insert(
            id.clone(),
            Entry {
                target,
                with: sign_in.with,
                flow: sign_in.flow,
                verifier,
                nonce,
                redirect,
                client,
                at: Instant::now(),
                status: SignInStatus::Waiting,
            },
        );
        Ok(SignInStart { id, url, mode })
    }

    /// Where a sign-in is; None for one unknown or old.
    pub fn status(&self, id: &str) -> Option<SignInStatus> {
        self.lock().get(id).filter(|entry| entry.at.elapsed() <= WAIT).map(|entry| entry.status.clone())
    }

    /// Takes a waiting sign-in for its exchange, once: a second return of it (a reload of the page, the same address
    /// pasted) is refused.
    pub fn take(&self, id: &str) -> Result<Taken, ApiError> {
        let mut entries = self.lock();
        let entry = entries.get_mut(id).filter(|entry| entry.at.elapsed() <= WAIT).ok_or_else(stale)?;
        match entry.status {
            SignInStatus::Waiting => {}
            SignInStatus::Working => return Err(ApiError::conflict("Вход уже завершается — подождите.")),
            SignInStatus::Done { .. } => return Err(ApiError::bad_request("Вход уже выполнен.")),
            SignInStatus::Failed { .. } => return Err(stale()),
        }
        entry.status = SignInStatus::Working;
        Ok(Taken {
            id: id.to_string(),
            target: entry.target.clone(),
            with: entry.with,
            flow: entry.flow,
            verifier: entry.verifier.clone(),
            nonce: entry.nonce.clone(),
            redirect: entry.redirect.clone(),
            client: entry.client.clone(),
        })
    }

    /// How a taken sign-in ended: the title of who got it, or why not.
    pub fn settle(&self, id: &str, outcome: &Result<String, ApiError>) {
        if let Some(entry) = self.lock().get_mut(id) {
            entry.status = match outcome {
                Ok(title) => SignInStatus::Done { title: title.clone() },
                Err(error) => SignInStatus::Failed { message: error.message.clone() },
            };
        }
    }

    /// The code of the return exchanged for what the service gives. The service's own words never reach the owner;
    /// the log gets the status and the error code.
    pub async fn exchange(&self, taken: &Taken, given: &Given) -> Result<Credential, ApiError> {
        match taken.flow {
            Flow::KeyExchange { exchange, .. } => {
                let body =
                    json!({ "code": given.code, "code_verifier": taken.verifier, "code_challenge_method": "S256" });
                let client = http_client().map_err(|_| refused(taken.with))?;
                let answer = call(client.post(exchange).json(&body)).await.map_err(|error| {
                    tracing::warn!(target: "oauth", "{}: the exchange did not go through: {error}", taken.with);
                    refused(taken.with)
                })?;
                match answer.get("key").and_then(Value::as_str).map(str::trim).filter(|key| plain_token(key, 512)) {
                    Some(key) => Ok(Credential::Key(key.to_string())),
                    None => {
                        tracing::warn!(target: "oauth", "{}: the exchange gave no key", taken.with);
                        Err(refused(taken.with))
                    }
                }
            }
            Flow::Tokens(flow) => {
                let registering = flow.registers && taken.client == flow.client;
                // A registration names the client it issued; a later sign-in names none, or the same one.
                let issued = match given.client_id.as_deref().map(str::trim).filter(|client| !client.is_empty()) {
                    Some(client) if registering || client == taken.client => client.to_string(),
                    Some(_) => {
                        tracing::warn!(target: "oauth", "{}: the return names another client", taken.with);
                        return Err(refused(taken.with));
                    }
                    None if registering => {
                        tracing::warn!(target: "oauth", "{}: the registration named no client", taken.with);
                        return Err(refused(taken.with));
                    }
                    None => taken.client.clone(),
                };
                if !plain_token(&issued, 256) {
                    return Err(refused(taken.with));
                }
                let mut form = vec![
                    ("grant_type", "authorization_code"),
                    ("client_id", issued.as_str()),
                    ("code", given.code.as_str()),
                    ("code_verifier", taken.verifier.as_str()),
                    ("redirect_uri", taken.redirect.as_str()),
                ];
                if let Some(resource) = flow.resource {
                    form.push(("resource", resource));
                }
                let client = http_client().map_err(|_| refused(taken.with))?;
                let answer = call(client.post(flow.token).form(&form)).await.map_err(|error| {
                    tracing::warn!(target: "oauth", "{}: the exchange did not go through: {error}", taken.with);
                    refused(taken.with)
                })?;
                TokenRecord::issued(&answer, &issued, &taken.nonce, flow, taken.with)
                    .map(Credential::Tokens)
                    .map_err(|why| ApiError::bad_request(format!("{why} Нажмите «Войти» ещё раз.")))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;

    use super::*;
    use crate::storage::Db;

    /// A service with OpenRouter's exchange: `good-code` with the verifier of the challenge it was told gives a key.
    #[derive(Clone, Default)]
    pub(crate) struct Service {
        pub challenge: Arc<Mutex<Option<String>>>,
        pub exchanges: Arc<Mutex<Vec<Value>>>,
    }

    pub(crate) const SERVICE_KEY: &str = "sk-or-v1-signed-in-key-5678";

    async fn exchange(State(service): State<Service>, Json(body): Json<Value>) -> axum::response::Response {
        service.exchanges.lock().unwrap().push(body.clone());
        let verifier = body["code_verifier"].as_str().unwrap_or_default();
        let expected = service.challenge.lock().unwrap().clone();
        if body["code"] != json!("good-code") || Some(challenge(verifier)) != expected {
            return (StatusCode::FORBIDDEN, r#"{"error":{"message":"Invalid code or code_verifier"}}"#).into_response();
        }
        Json(json!({ "key": SERVICE_KEY, "user_id": "user_1" })).into_response()
    }

    /// The service on a free port, and its sign-in as a preset would name it.
    pub(crate) async fn service() -> (Service, SignIn) {
        let service = Service::default();
        let app =
            axum::Router::new().route("/api/v1/auth/keys", axum::routing::post(exchange)).with_state(service.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let flow = Flow::KeyExchange {
            authorize: Box::leak(format!("{base}/auth").into_boxed_str()),
            exchange: Box::leak(format!("{base}/api/v1/auth/keys").into_boxed_str()),
        };
        (service, SignIn { with: "Fake", label: None, flow })
    }

    /// The service learns the challenge of a page, as its own sign-in page would.
    pub(crate) fn shown(service: &Service, url: &str) {
        *service.challenge.lock().unwrap() = param(url, "code_challenge");
    }

    pub(crate) fn param(url: &str, name: &str) -> Option<String> {
        url::Url::parse(url).unwrap().query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
    }

    fn provider() -> Target {
        Target::Provider { id: "openrouter".into(), created_at: "2026-11-12T10:00:00.000Z".into() }
    }

    pub(crate) fn engine() -> (OAuthService, Arc<SecretsService>) {
        let secrets = Arc::new(SecretsService::new(Arc::new(Db::memory()), None).unwrap());
        (OAuthService::new(7780, secrets.clone()), secrets)
    }

    #[test]
    fn makes_the_challenge_as_rfc_7636_does() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let made = verifier();
        assert_eq!(made.len(), 43);
        assert_ne!(made, verifier());
    }

    #[test]
    fn returns_to_this_computer() {
        assert_eq!(return_url(7780, CALLBACK_PATH), "http://localhost:7780/api/v1/oauth/callback");
        assert_eq!(return_url(80, "/x"), "http://localhost/x");
        assert_eq!(loopback_url(7780, "/auth/callback"), "http://127.0.0.1:7780/auth/callback");
        assert_eq!(loopback_url(80, "/auth/callback"), "http://127.0.0.1/auth/callback");
    }

    #[test]
    fn sends_the_owner_back_here_or_asks_the_page_for_a_code() {
        let (engine, _) = engine();
        let sign_in = SignIn {
            with: "OpenRouter",
            label: None,
            flow: Flow::KeyExchange {
                authorize: "https://openrouter.ai/auth",
                exchange: "https://openrouter.ai/api/v1/auth/keys",
            },
        };
        let here = engine.start(&sign_in, provider(), true).unwrap();
        assert!(here.url.starts_with("https://openrouter.ai/auth?"), "{}", here.url);
        assert_eq!(param(&here.url, "callback_url").as_deref(), Some("http://localhost:7780/api/v1/oauth/callback"));
        assert_eq!(param(&here.url, "state"), Some(here.id.clone()));
        assert_eq!(param(&here.url, "code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(param(&here.url, "code_challenge").map(|c| c.len()), Some(43));
        assert_eq!(param(&here.url, "key_label").as_deref(), Some("DuoWave"));
        assert_eq!((here.id.len(), here.mode), (32, Return::Here));

        let code = engine.start(&sign_in, provider(), false).unwrap();
        assert_eq!((param(&code.url, "callback_url"), param(&code.url, "state")), (None, None));
        assert!(param(&code.url, "code_challenge").is_some());
        assert_ne!(code.id, here.id);
        assert_eq!(engine.status(&code.id), Some(SignInStatus::Waiting));
        assert_eq!(serde_json::to_value(&code).unwrap()["mode"], json!("code"));
        assert_eq!(serde_json::to_value(sign_in).unwrap(), json!({"with": "OpenRouter"}));
    }

    #[tokio::test]
    async fn exchanges_the_code_once_with_the_verifier_of_its_challenge() {
        let (service, sign_in) = service().await;
        let (engine, _) = engine();
        let started = engine.start(&sign_in, provider(), false).unwrap();
        shown(&service, &started.url);

        let taken = engine.take(&started.id).unwrap();
        assert_eq!(engine.status(&started.id), Some(SignInStatus::Working));
        assert_eq!(engine.take(&started.id).err().unwrap().message, "Вход уже завершается — подождите.");
        let given = Given { code: "good-code".into(), client_id: None };
        let Ok(Credential::Key(key)) = engine.exchange(&taken, &given).await else { panic!("a key") };
        assert_eq!(key, SERVICE_KEY);
        let sent = service.exchanges.lock().unwrap()[0].clone();
        assert_eq!(sent["code_challenge_method"], json!("S256"));
        engine.settle(&started.id, &Ok("OpenRouter".into()));
        assert_eq!(engine.status(&started.id), Some(SignInStatus::Done { title: "OpenRouter".into() }));
        assert_eq!(engine.take(&started.id).err().unwrap().message, "Вход уже выполнен.");
        assert!(engine.take("forged").err().unwrap().message.contains("устарел"));
    }

    #[tokio::test]
    async fn a_code_the_service_refuses_brings_nothing_and_the_owner_reads_why_in_our_words() {
        let (service, sign_in) = service().await;
        let (engine, _) = engine();
        let started = engine.start(&sign_in, provider(), true).unwrap();
        shown(&service, &started.url);
        let taken = engine.take(&started.id).unwrap();
        let given = Given { code: "stolen-code".into(), client_id: None };
        let Err(refused) = engine.exchange(&taken, &given).await else { panic!("refused") };
        assert_eq!(refused.message, "Войти не получилось: сервис Fake не подтвердил вход. Нажмите «Войти» ещё раз.");
        assert!(!refused.message.contains("Invalid"));
        engine.settle(&started.id, &Err(refused));
        assert!(matches!(engine.status(&started.id), Some(SignInStatus::Failed { .. })));
        assert!(engine.take(&started.id).err().unwrap().message.contains("устарел"));
    }

    #[test]
    fn takes_only_plain_tokens() {
        assert!(plain_token("sk-or-v1-abc", 512));
        assert!(!plain_token("", 512));
        assert!(!plain_token("two words", 512));
        assert!(!plain_token("line\nbreak", 512));
        assert!(!plain_token("ключ", 512));
        assert!(!plain_token(&"x".repeat(513), 512));
    }

    /// ChatGPT's way, against a service of our own: tokens.rs's issuer answers the code too.
    fn token_sign_in(base: &str) -> SignIn {
        SignIn { with: "ChatGPT", label: Some("Continue with ChatGPT"), flow: Flow::Tokens(tokens::tests::flow(base)) }
    }

    fn chatgpt() -> Target {
        Target::Provider { id: "chatgpt".into(), created_at: "2026-11-12T10:00:00.000Z".into() }
    }

    #[test]
    fn registers_at_the_first_sign_in_and_signs_in_again_with_the_issued_client() {
        let (engine, secrets) = engine();
        let sign_in = token_sign_in("https://auth.example");
        let first = engine.start(&sign_in, chatgpt(), true).unwrap();
        let get = |name: &str| param(&first.url, name);
        assert!(first.url.starts_with("https://auth.example/authorize?"), "{}", first.url);
        assert_eq!(get("client_id").as_deref(), Some("dynamic_agent_client"));
        assert_eq!(get("agent_name_hint").as_deref(), Some("DuoWave"));
        assert_eq!(get("redirect_uri").as_deref(), Some("http://127.0.0.1:7780/auth/callback"));
        assert_eq!(get("response_type").as_deref(), Some("code"));
        assert_eq!(get("scope").as_deref(), Some("openid email offline_access plan.use"));
        assert_eq!(get("resource").as_deref(), Some("https://api.example/v1"));
        assert_eq!(get("state"), Some(first.id.clone()));
        assert_eq!(get("nonce").map(|nonce| nonce.len()), Some(32));
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        let host = get("ext_agent_host_id").unwrap();
        assert!(host.starts_with("urn:uuid:"), "{host}");
        assert_eq!(first.mode, Return::Here);
        assert_eq!(engine.start(&sign_in, chatgpt(), false).unwrap().mode, Return::Address);
        assert_eq!(
            serde_json::to_value(sign_in).unwrap(),
            json!({"with": "ChatGPT", "label": "Continue with ChatGPT", "keyless": true})
        );

        // Signed in once: the next sign-in goes with the issued client and says whose it was; the host stays the same.
        let earlier = TokenRecord {
            client_id: "oaiapp_1".into(),
            id_token: "earlier.id.token".into(),
            email: Some("owner@example.com".into()),
            ..TokenRecord::default()
        };
        secrets.set("oauth:llm:chatgpt", Some(&serde_json::to_string(&earlier).unwrap())).unwrap();
        let again = engine.start(&sign_in, chatgpt(), true).unwrap();
        let get = |name: &str| param(&again.url, name);
        assert_eq!(get("client_id").as_deref(), Some("oaiapp_1"));
        assert_eq!(get("agent_name_hint"), None);
        assert_eq!(get("id_token_hint").as_deref(), Some("earlier.id.token"));
        assert_eq!(get("login_hint").as_deref(), Some("owner@example.com"));
        assert_eq!(get("ext_agent_host_id"), Some(host));
        assert!(!secrets.ids("oauth:host").is_empty() && !secrets.ids("llm:").iter().any(|id| id.contains("oauth")));
    }

    #[tokio::test]
    async fn the_registration_brings_tokens_for_the_client_it_names_and_only_with_the_plan() {
        let (issuer, base) = tokens::tests::issuer().await;
        let (engine, _) = engine();
        let sign_in = token_sign_in(&base);
        let started = engine.start(&sign_in, chatgpt(), true).unwrap();
        *issuer.nonce.lock().unwrap() = param(&started.url, "nonce");

        // A registration that names no client is not one.
        let taken = engine.take(&started.id).unwrap();
        let nameless = engine.exchange(&taken, &Given { code: "good-code".into(), client_id: None }).await;
        assert!(nameless.err().unwrap().message.contains("не подтвердил вход"));

        let started = engine.start(&sign_in, chatgpt(), true).unwrap();
        *issuer.nonce.lock().unwrap() = param(&started.url, "nonce");
        let taken = engine.take(&started.id).unwrap();
        let given = Given { code: "good-code".into(), client_id: Some("oaiapp_9".into()) };
        let Ok(Credential::Tokens(record)) = engine.exchange(&taken, &given).await else { panic!("tokens") };
        assert_eq!((record.client_id.as_str(), record.access_token.as_str()), ("oaiapp_9", "access-code"));
        assert_eq!(
            (record.refresh_token.as_str(), record.email.as_deref()),
            ("refresh-code", Some("owner@example.com"))
        );
        let form = issuer.forms.lock().unwrap().last().unwrap().clone();
        let field = |name: &str| form.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(field("grant_type").as_deref(), Some("authorization_code"));
        assert_eq!(field("client_id").as_deref(), Some("oaiapp_9"));
        assert_eq!(field("redirect_uri").as_deref(), Some("http://127.0.0.1:7780/auth/callback"));
        assert_eq!(field("resource").as_deref(), Some("https://api.example/v1"));
        assert_eq!(field("code_verifier").map(|verifier| challenge(&verifier)), param(&started.url, "code_challenge"));

        // The plan not granted: no tokens, and the owner reads what to do.
        *issuer.scope.lock().unwrap() = Some("openid email offline_access".into());
        let started = engine.start(&sign_in, chatgpt(), true).unwrap();
        *issuer.nonce.lock().unwrap() = param(&started.url, "nonce");
        let taken = engine.take(&started.id).unwrap();
        let given = Given { code: "good-code".into(), client_id: Some("oaiapp_9".into()) };
        let refused = engine.exchange(&taken, &given).await.err().unwrap();
        assert!(refused.message.contains("без разрешения отвечать из подписки"), "{}", refused.message);
    }
}
