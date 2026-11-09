//! Signing in to a server in the cloud (Todoist, Notion): the gateway is the OAuth client. The SDK (rmcp) does the
//! protocol — discovery, registration (dynamic, or the client of an earlier sign-in at the same authorization
//! server), PKCE, the code exchange, refreshing; this module keeps what it must remember sealed among the secrets
//! (`mcp:<id>:oauth:tokens`: the tokens, the client id and the issuer that minted them) and sends the OAuth requests
//! over the gateway's own TLS. The return comes to localhost: on this computer the browser lands on the gateway by
//! itself; from a phone the owner pastes the address the browser could not open.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore,
    OAuthHttpClient, OAuthHttpClientError, OAuthHttpClientFuture, OAuthHttpRedirectPolicy, OAuthHttpRequest,
    StoredCredentials,
};

use crate::secrets::SecretsService;
use crate::utils::http::client_builder;

const CLIENT_NAME: &str = "DuoWave";
/// An OAuth request's longest wait, as the SDK's own client has it.
const OAUTH_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest OAuth answer read (metadata, a registration, tokens).
const BODY_LIMIT: usize = 1024 * 1024;

pub fn prefix(server_id: &str) -> String {
    format!("mcp:{server_id}:oauth:")
}

fn tokens_id(server_id: &str) -> String {
    format!("{}tokens", prefix(server_id))
}

/// Whether a server has tokens of a past sign-in.
pub fn signed_in(secrets: &SecretsService, server_id: &str) -> bool {
    secrets.has(&tokens_id(server_id))
}

/// Where the authorization server sends the browser back: the gateway on this computer.
pub fn callback_url(port: u16) -> String {
    let host = if port == 0 || port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}/api/v1/mcp/oauth/callback")
}

/// The tokens of a server, sealed: what rmcp keeps between sign-ins and refreshes.
pub struct SealedCredentials {
    secrets: Arc<SecretsService>,
    server_id: String,
}

impl SealedCredentials {
    pub fn new(secrets: Arc<SecretsService>, server_id: &str) -> SealedCredentials {
        SealedCredentials { secrets, server_id: server_id.to_string() }
    }

    fn read(&self) -> Option<StoredCredentials> {
        serde_json::from_str(&self.secrets.get(&tokens_id(&self.server_id))?).ok()
    }
}

fn store_error(error: impl std::fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(error.to_string())
}

#[async_trait]
impl CredentialStore for SealedCredentials {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self.read())
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let text = serde_json::to_string(&credentials).map_err(store_error)?;
        self.secrets.set(&tokens_id(&self.server_id), Some(&text)).map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.secrets.delete(&tokens_id(&self.server_id)).map_err(store_error)
    }

    /// The service no longer takes the refresh token: the server waits for «Войти» again.
    async fn on_refresh_token_rejected(&self, _credentials: &StoredCredentials) -> Result<(), AuthError> {
        self.clear().await
    }
}

/// The OAuth requests over the gateway's TLS (the same on the station and a PC). A request the SDK asks not to
/// redirect (the code exchange, a refresh) never follows one: the code and the verifier go only where discovery said.
pub struct GatewayOAuthHttp {
    follow: reqwest::Client,
    stop: reqwest::Client,
}

impl GatewayOAuthHttp {
    pub fn new() -> Result<GatewayOAuthHttp, AuthError> {
        let build = |policy| {
            client_builder()
                .timeout(OAUTH_TIMEOUT)
                .redirect(policy)
                .build()
                .map_err(|error| AuthError::InternalError(error.to_string()))
        };
        Ok(GatewayOAuthHttp {
            follow: build(reqwest::redirect::Policy::limited(10))?,
            stop: build(reqwest::redirect::Policy::none())?,
        })
    }
}

impl OAuthHttpClient for GatewayOAuthHttp {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let OAuthHttpRequest { request, redirect_policy, .. } = request;
            let client = match redirect_policy {
                OAuthHttpRedirectPolicy::Stop => &self.stop,
                _ => &self.follow,
            };
            let request = reqwest::Request::try_from(request).map_err(|e| Box::new(e) as OAuthHttpClientError)?;
            let response = client.execute(request).await.map_err(|e| Box::new(e) as OAuthHttpClientError)?;
            let mut builder = axum::http::Response::builder().status(response.status()).version(response.version());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let mut body = Vec::new();
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|e| Box::new(e) as OAuthHttpClientError)?;
                if body.len() + chunk.len() > BODY_LIMIT {
                    return Err("the OAuth answer is too large".into());
                }
                body.extend_from_slice(&chunk);
            }
            builder.body(body).map_err(|e| Box::new(e) as OAuthHttpClientError)
        })
    }
}

/// An authorization manager for a server: our HTTP, our sealed tokens.
async fn manager(url: &str, server_id: &str, secrets: &Arc<SecretsService>) -> Result<AuthorizationManager, AuthError> {
    let mut manager = AuthorizationManager::new_with_oauth_http_client(url, Arc::new(GatewayOAuthHttp::new()?)).await?;
    manager.set_credential_store(SealedCredentials::new(secrets.clone(), server_id));
    Ok(manager)
}

/// What a connection needs to sign its requests with a server's sealed tokens (refreshed when they run out).
#[derive(Clone)]
pub struct OAuthLink {
    pub server_id: String,
    secrets: Arc<SecretsService>,
}

impl OAuthLink {
    pub fn new(server_id: &str, secrets: Arc<SecretsService>) -> OAuthLink {
        OAuthLink { server_id: server_id.to_string(), secrets }
    }

    /// The HTTP client of the MCP requests with the tokens; AuthorizationRequired without a past sign-in.
    pub async fn client(&self, url: &str) -> Result<AuthClient<reqwest::Client>, AuthError> {
        let mut manager = manager(url, &self.server_id, &self.secrets).await?;
        if !manager.initialize_from_store().await? {
            return Err(AuthError::AuthorizationRequired);
        }
        Ok(AuthClient::new(crate::utils::http::client(), manager))
    }

    /// A fresh access token, for the old SSE transport that sends it as a header.
    pub async fn access_token(&self, url: &str) -> Result<String, AuthError> {
        self.client(url).await?.get_access_token().await
    }
}

/// A sign-in begun: the code from the return finishes it with what discovery found.
pub struct PendingLogin {
    session: AuthorizationSession,
    /// The page to send the owner to.
    pub url: String,
    /// This sign-in's state: the return finds the sign-in by it.
    pub state: String,
}

impl PendingLogin {
    /// The code exchanged for tokens (the SDK checks the issuer the return names); the tokens are sealed.
    pub async fn finish(&self, code: &str, state: &str, issuer: Option<&str>) -> Result<(), AuthError> {
        self.session.handle_callback_with_issuer(code, state, issuer).await.map(|_| ())
    }
}

/// Starts signing in to a server by address: discovery (seeded by the server's 401 challenge when there is one),
/// the client (the one registered at the last sign-in with the same authorization server, or a new registration)
/// and the page for the owner.
pub async fn start_login(
    url: &str,
    server_id: &str,
    secrets: &Arc<SecretsService>,
    callback: &str,
    challenge: Option<&str>,
) -> Result<PendingLogin, AuthError> {
    let mut manager = manager(url, server_id, secrets).await?;
    let resolution = manager.resolve_metadata_from_challenge(challenge).await?;
    let issuer = resolution.metadata.issuer.clone();
    manager.set_metadata(resolution.metadata);
    let mut request = AuthorizationRequest::new(callback).with_client_name(CLIENT_NAME).with_application_type("native");
    if let Some(challenge) = challenge {
        request = request.with_challenge(challenge);
    }
    let earlier = SealedCredentials::new(secrets.clone(), server_id).read();
    if let Some(earlier) = earlier.filter(|earlier| earlier.issuer.is_some() && earlier.issuer == issuer) {
        request = request.with_preregistered_client(earlier.client_id);
    }
    let session = AuthorizationSession::new(manager, request).await.map_err(|(_, error)| error)?;
    let page = session.get_authorization_url().to_string();
    let state = url::Url::parse(&page)
        .ok()
        .and_then(|parsed| parsed.query_pairs().find(|(key, _)| key == "state").map(|(_, value)| value.into_owned()))
        .ok_or_else(|| AuthError::AuthorizationFailed("the authorization page has no state".into()))?;
    Ok(PendingLogin { session, url: page, state })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::mcp::fixtures;
    use crate::mcp::mcp_service::tests::{input, ready, setup, state, until};
    use crate::mcp::mcp_service::ServerState;

    #[test]
    fn sends_the_browser_back_to_this_computer() {
        assert_eq!(callback_url(9930), "http://localhost:9930/api/v1/mcp/oauth/callback");
        assert_eq!(callback_url(80), "http://localhost/api/v1/mcp/oauth/callback");
    }

    #[tokio::test]
    async fn waits_for_sign_in_sends_the_owner_to_the_service_takes_the_code_back_to_localhost_and_connects_with_sealed_tokens(
    ) {
        let cloud = fixtures::cloud().await;
        let s = setup();
        let server = s
            .mcp
            .add(&input(json!({ "transport": "http", "title": "Cloud", "url": format!("{}/mcp", cloud.base), "auth": "oauth" })), None)
            .unwrap();
        until("«Войти»", || state(&s.mcp, &server.id).state == ServerState::Login, Duration::from_secs(5)).await;

        let started = s.mcp.login(&server.id).await.unwrap();
        let url = started.url.expect("the service's page");
        let page = url::Url::parse(&url).unwrap();
        assert_eq!(
            format!("{}{}", page.origin().ascii_serialization(), page.path()),
            format!("{}/authorize", cloud.base)
        );
        let param = |name: &str| page.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
        assert_eq!(param("redirect_uri").as_deref(), Some("http://localhost:9930/api/v1/mcp/oauth/callback"));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));

        // The browser signs in and is sent back; the callback hands us the address it was sent to.
        let browser = client_builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let back = browser.get(&url).send().await.unwrap();
        let returned = url::Url::parse(back.headers()[reqwest::header::LOCATION].to_str().unwrap()).unwrap();
        let query: Vec<(String, String)> =
            returned.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let forged: Vec<(String, String)> = query
            .iter()
            .map(|(key, value)| (key.clone(), if key == "state" { "forged".into() } else { value.clone() }))
            .collect();
        let refused = s.mcp.complete_login(&forged, None).await.unwrap_err();
        assert!(refused.message.contains("устарел или начат не здесь"), "{}", refused.message);
        let done = s.mcp.complete_login(&query, None).await.unwrap();
        assert_eq!((done.id.as_str(), done.title.as_str()), (server.id.as_str(), "Cloud"));
        ready(&s.mcp, &server.id).await;
        let tools: Vec<String> = state(&s.mcp, &server.id).tools.into_iter().map(|tool| tool.name).collect();
        assert_eq!(tools, ["tasks"]);
        assert_eq!(cloud.issued.lock().unwrap().as_slice(), ["token-1"]);
        assert!(s.secrets.ids(&prefix(&server.id)).contains(&format!("mcp:{}:oauth:tokens", server.id)));
        assert!(!serde_json::to_string(&s.mcp.view()).unwrap().contains("token-1"));
        // The same return cannot be used twice.
        let again = s.mcp.complete_login(&query, None).await.unwrap_err();
        assert!(again.message.contains("устарел"), "{}", again.message);
        // Signed in already: «Войти» has no page to open, the server just reconnects.
        let started = s.mcp.login(&server.id).await.unwrap();
        assert_eq!((started.url, started.done), (None, Some(true)));
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn keeps_a_sign_in_whose_page_was_closed_while_the_code_was_exchanged() {
        let cloud = fixtures::cloud_exchanging_after(Duration::from_millis(300)).await;
        let s = setup();
        let given =
            json!({ "transport": "http", "title": "Cloud", "url": format!("{}/mcp", cloud.base), "auth": "oauth" });
        let server = s.mcp.add(&input(given), None).unwrap();
        until("«Войти»", || state(&s.mcp, &server.id).state == ServerState::Login, Duration::from_secs(5)).await;
        let url = s.mcp.login(&server.id).await.unwrap().url.expect("the service's page");
        let browser = client_builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let back = browser.get(&url).send().await.unwrap();
        let returned = url::Url::parse(back.headers()[reqwest::header::LOCATION].to_str().unwrap()).unwrap();
        let query: Vec<(String, String)> =
            returned.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        // The return's page is closed during the exchange: the request, and what it awaits, is dropped.
        let left = tokio::time::timeout(Duration::from_millis(50), s.mcp.complete_login(&query, None)).await;
        assert!(left.is_err(), "the exchange outlasted the page");
        // The sign-in is not lost: the tokens are kept and the server connects.
        ready(&s.mcp, &server.id).await;
        assert_eq!(cloud.issued.lock().unwrap().as_slice(), ["token-1"]);
        assert!(s.secrets.ids(&prefix(&server.id)).contains(&format!("mcp:{}:oauth:tokens", server.id)));
        s.mcp.shutdown().await;
    }
}
