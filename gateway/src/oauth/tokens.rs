//! Tokens a sign-in keeps (src/oauth): OAuth 2.0 with an access token that lives about an hour and a refresh token
//! replaced at every renewal — ChatGPT's plan («Sign in with ChatGPT»), later Hugging Face. The record stays on this
//! gateway (`oauth:llm:<provider>`, not among the travelling secrets of src/placement/transfer.rs): one refresh token
//! renewed on two computers would end the sign-in on one of them.
//!
//! `TokenAccess` gives a client its bearer before each request: renewed a little before it runs out, one renewal for
//! everyone waiting, and the replacement kept before the new token is used. A renewal the service refuses for good
//! ends the sign-in: the tokens go, the registration stays for the next «Войти»; a network failure keeps them.

use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::anyhow;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{call, http_client, plain_token, CallError};
use crate::dialog::llm::{AccessSource, SpokenFailure};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::utils::time::{now_iso, now_ms};

/// How long before its end a token is renewed, ms.
const MARGIN_MS: i64 = 2 * 60 * 1000;
/// A token's life when the service names none, s.
const DEFAULT_LIFE_S: i64 = 3600;
/// The renewal errors after which only a new sign-in helps (OpenAI's «Refresh errors»).
const TERMINAL: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

/// A sign-in with tokens, as a preset names it.
#[derive(Debug, PartialEq, Eq)]
pub struct TokenFlow {
    pub authorize: &'static str,
    pub token: &'static str,
    pub revoke: Option<&'static str>,
    /// Who issues the ID token (`iss`).
    pub issuer: &'static str,
    /// The client at the first sign-in.
    pub client: &'static str,
    /// `client` asks the service to register one on the fly: the return names the issued client, kept for every later
    /// sign-in and renewal (ChatGPT's `dynamic_agent_client`).
    pub registers: bool,
    pub scope: &'static str,
    pub resource: Option<&'static str>,
    /// Granted, or the sign-in brings nothing the provider can use (ChatGPT's plan usage).
    pub required_scope: Option<&'static str>,
    /// The return at `http://127.0.0.1:<port><path>` (ChatGPT takes no other); None: the gateway's own return.
    pub loopback: Option<&'static str>,
    /// The service wants this gateway's stable id (`ext_agent_host_id`) and, at the first sign-in, our name.
    pub host_id: bool,
}

/// What a sign-in keeps: sealed among the secrets, never logged, never in an answer.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRecord {
    pub client_id: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// When the access token ends, ms since the epoch.
    #[serde(default)]
    pub expires_at: i64,
    /// Not renewed before this, ms since the epoch (0: any time).
    #[serde(default)]
    pub earliest_refresh_at: i64,
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub saved_at: String,
}

/// For tests and logs: whether the tokens are there, never what they are.
impl std::fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("client_id", &self.client_id)
            .field("access_token", &!self.access_token.is_empty())
            .field("refresh_token", &!self.refresh_token.is_empty())
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl TokenRecord {
    /// Signed in: a token to use or one to renew.
    pub fn signed_in(&self) -> bool {
        !self.client_id.is_empty() && (!self.access_token.is_empty() || !self.refresh_token.is_empty())
    }

    fn fresh(&self, now: i64) -> bool {
        !self.access_token.is_empty() && self.expires_at - now > MARGIN_MS
    }

    fn alive(&self, now: i64) -> bool {
        !self.access_token.is_empty() && self.expires_at > now
    }

    /// The sign-in over: the registration and who it was stay, for the next «Войти».
    fn signed_out(&self) -> TokenRecord {
        TokenRecord {
            client_id: self.client_id.clone(),
            id_token: self.id_token.clone(),
            email: self.email.clone(),
            subject: self.subject.clone(),
            saved_at: now_iso(),
            ..TokenRecord::default()
        }
    }

    /// The record of a code's exchange: the tokens, who signed in (the ID token, its nonce checked) and what was
    /// granted. Why not, in words for the owner.
    pub fn issued(
        answer: &Value,
        client_id: &str,
        nonce: &str,
        flow: &TokenFlow,
        with: &str,
    ) -> Result<TokenRecord, String> {
        let text = |name: &str| answer.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default();
        let access = text("access_token");
        if !plain_token(access, 16 * 1024) {
            return Err(crate::tr!("Сервис {with} не выдал токен.", with = with));
        }
        let scope = Some(text("scope")).filter(|scope| !scope.is_empty()).unwrap_or(flow.scope).to_string();
        if let Some(required) = flow.required_scope {
            if !scope.split_whitespace().any(|granted| granted == required) {
                return Err(crate::tr!(
                    "Вход в {with} выполнен, но без разрешения отвечать из подписки: при входе разрешите это DuoWave.",
                    with = with
                ));
            }
        }
        let mut record = TokenRecord {
            client_id: client_id.to_string(),
            access_token: access.to_string(),
            refresh_token: Some(text("refresh_token"))
                .filter(|token| plain_token(token, 16 * 1024))
                .unwrap_or_default()
                .to_string(),
            scope,
            saved_at: now_iso(),
            ..TokenRecord::default()
        };
        record.times(answer);
        let id_token = text("id_token");
        if !id_token.is_empty() {
            let claims =
                claims(id_token).ok_or_else(|| crate::tr!("Сервис {with} выдал непонятный ID-токен.", with = with))?;
            let says = |name: &str| claims.get(name).and_then(Value::as_str);
            if says("nonce") != Some(nonce) || says("iss") != Some(flow.issuer) {
                return Err(crate::tr!("Сервис {with} выдал чужой ID-токен.", with = with));
            }
            let audience = claims.get("aud");
            let ours = audience.and_then(Value::as_str) == Some(client_id)
                || audience
                    .and_then(Value::as_array)
                    .is_some_and(|all| all.iter().any(|aud| aud.as_str() == Some(client_id)));
            if audience.is_some() && !ours {
                tracing::warn!(target: "oauth", "{with}: the ID token names another audience");
            }
            record.id_token = id_token.to_string();
            record.email = says("email").map(str::to_string);
            record.subject = says("sub").map(str::to_string);
        }
        Ok(record)
    }

    /// The record after a renewal: the new access token, the replacement refresh token (or the same one when none
    /// came), the new times; an ID token of another person ends it.
    fn renewed(&self, answer: &Value) -> Result<TokenRecord, String> {
        let text = |name: &str| answer.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default();
        let access = text("access_token");
        if !plain_token(access, 16 * 1024) {
            return Err("the renewal gave no access token".into());
        }
        let mut next = self.clone();
        next.access_token = access.to_string();
        if plain_token(text("refresh_token"), 16 * 1024) {
            next.refresh_token = text("refresh_token").to_string();
        }
        if !text("scope").is_empty() {
            next.scope = text("scope").to_string();
        }
        next.earliest_refresh_at = 0;
        next.times(answer);
        let id_token = text("id_token");
        if let Some(claims) = Some(id_token).filter(|token| !token.is_empty()).and_then(claims) {
            let subject = claims.get("sub").and_then(Value::as_str);
            if subject.is_some() && self.subject.is_some() && subject != self.subject.as_deref() {
                return Err("the renewal named another person".into());
            }
            next.id_token = id_token.to_string();
            if let Some(email) = claims.get("email").and_then(Value::as_str) {
                next.email = Some(email.to_string());
            }
        }
        next.saved_at = now_iso();
        Ok(next)
    }

    /// The end and the earliest renewal from an answer: `expires_in` seconds; `earliest_refresh_at` as seconds,
    /// milliseconds or a time.
    fn times(&mut self, answer: &Value) {
        let now = now_ms() as i64;
        let life = answer.get("expires_in").and_then(Value::as_i64).filter(|life| *life > 0).unwrap_or(DEFAULT_LIFE_S);
        self.expires_at = now + life * 1000;
        self.earliest_refresh_at = match answer.get("earliest_refresh_at") {
            Some(Value::Number(at)) => {
                at.as_i64().map(|at| if at < 100_000_000_000 { at * 1000 } else { at }).unwrap_or(0)
            }
            Some(Value::String(at)) => {
                chrono::DateTime::parse_from_rfc3339(at).map(|at| at.timestamp_millis()).unwrap_or(0)
            }
            _ => 0,
        };
    }
}

/// An ID token's claims, read without its signature: it came straight from the service's token endpoint over TLS,
/// which OpenID Connect allows to stand for it (Core 1.0, 3.1.3.7).
pub fn claims(id_token: &str) -> Option<Value> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What stands in the way of a provider signed in this way, for its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Problem {
    /// The plan's limit is reached: the service said so on the last request.
    Limit,
    /// The sign-in is over (renewal refused, or never done): «Войти» again.
    Signin,
}

/// A sign-in as the panel shows it: whether there is one, whose (the e-mail half hidden) and what stands in the way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

/// `d…@example.com`: enough to tell one account from another, not the whole address.
fn masked(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => format!("{}…@{domain}", name.chars().next().unwrap_or('…')),
        None => "…".into(),
    }
}

/// The bearer of a provider signed in with tokens, for its clients (src/dialog/llm).
pub struct TokenAccess {
    secrets: Arc<SecretsService>,
    id: String,
    flow: &'static TokenFlow,
    with: &'static str,
    renewing: tokio::sync::Mutex<()>,
    problem: Mutex<Option<Problem>>,
}

impl TokenAccess {
    pub fn new(secrets: Arc<SecretsService>, id: &str, flow: &'static TokenFlow, with: &'static str) -> TokenAccess {
        TokenAccess {
            secrets,
            id: id.to_string(),
            flow,
            with,
            renewing: tokio::sync::Mutex::new(()),
            problem: Mutex::new(None),
        }
    }

    pub fn record(&self) -> Option<TokenRecord> {
        serde_json::from_str(&self.secrets.get(&self.id)?).ok()
    }

    fn save(&self, record: &TokenRecord) -> Result<(), ApiError> {
        let text = serde_json::to_string(record).map_err(ApiError::internal)?;
        self.secrets.set(&self.id, Some(&text))
    }

    fn problem(&self) -> MutexGuard<'_, Option<Problem>> {
        self.problem.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A new sign-in came: what stood in the way is over.
    pub fn reset(&self) {
        *self.problem() = None;
    }

    pub fn account(&self) -> AccountView {
        let record = self.record();
        let signed_in = record.as_ref().is_some_and(TokenRecord::signed_in);
        let problem = if signed_in { *self.problem() } else { Some(Problem::Signin) };
        AccountView { signed_in, who: record.and_then(|record| record.email).map(|email| masked(&email)), problem }
    }

    /// Said by the station, and on the card, when only a new sign-in helps.
    fn signed_out(&self, detail: impl Into<String>) -> anyhow::Error {
        *self.problem() = Some(Problem::Signin);
        anyhow::Error::new(SpokenFailure {
            phrase: format!("Вход в {} закончился. Войдите снова в пульте, в «Мозге».", self.with),
            detail: format!("{}: {}", self.with, detail.into()),
        })
    }

    /// Never signed in yet: the same, in other words.
    fn not_signed_in(&self) -> anyhow::Error {
        anyhow::Error::new(SpokenFailure {
            phrase: format!("Нужно войти в {}: в пульте, в «Мозге».", self.with),
            detail: format!("{}: no sign-in", self.with),
        })
    }

    async fn fresh(&self) -> anyhow::Result<String> {
        let usable = |record: &TokenRecord| record.signed_in();
        let record = self.record().filter(usable).ok_or_else(|| self.not_signed_in())?;
        if record.fresh(now_ms() as i64) {
            return Ok(record.access_token);
        }
        // One renewal at a time: a refresh token renewed twice would be refused the second time.
        let _turn = self.renewing.lock().await;
        let record = self.record().filter(usable).ok_or_else(|| self.not_signed_in())?;
        let now = now_ms() as i64;
        if record.fresh(now) || (record.earliest_refresh_at > now && record.alive(now)) {
            return Ok(record.access_token);
        }
        if record.refresh_token.is_empty() {
            self.save(&record.signed_out()).map_err(|error| anyhow!(error.message))?;
            return Err(self.signed_out("the token ran out and there is nothing to renew it with"));
        }
        match renew(self.flow, &record).await {
            Ok(answer) => match record.renewed(&answer) {
                Ok(next) => {
                    // Kept before it is used: the old refresh token is gone at the service.
                    self.save(&next).map_err(|error| anyhow!(error.message))?;
                    if *self.problem() == Some(Problem::Signin) {
                        *self.problem() = None;
                    }
                    Ok(next.access_token)
                }
                Err(why) => {
                    self.save(&record.signed_out()).map_err(|error| anyhow!(error.message))?;
                    Err(self.signed_out(why))
                }
            },
            Err(error) if error.code.as_deref().is_some_and(|code| TERMINAL.contains(&code)) => {
                self.save(&record.signed_out()).map_err(|error| anyhow!(error.message))?;
                Err(self.signed_out(format!("the renewal was refused: {error}")))
            }
            Err(error) if record.alive(now) => {
                tracing::warn!(target: "oauth", "{}: cannot renew the sign-in yet: {error}", self.with);
                Ok(record.access_token)
            }
            Err(error) => Err(anyhow!("{}: cannot renew the sign-in: {error}", self.with)),
        }
    }
}

impl AccessSource for TokenAccess {
    fn bearer(&self) -> BoxFuture<'_, anyhow::Result<String>> {
        Box::pin(self.fresh())
    }

    fn ok(&self) {
        let mut problem = self.problem();
        if *problem == Some(Problem::Limit) {
            *problem = None;
        }
    }

    fn limited(&self) {
        *self.problem() = Some(Problem::Limit);
    }
}

async fn renew(flow: &TokenFlow, record: &TokenRecord) -> Result<Value, CallError> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("client_id", record.client_id.as_str()),
        ("refresh_token", record.refresh_token.as_str()),
    ];
    if let Some(resource) = flow.resource {
        form.push(("resource", resource));
    }
    call(http_client()?.post(flow.token).form(&form)).await
}

/// Ends a sign-in at the service (its provider removed): the refresh token revoked. Whether the service said so.
pub async fn revoke(flow: &TokenFlow, record: &TokenRecord) -> bool {
    let Some(url) = flow.revoke.filter(|_| !record.refresh_token.is_empty()) else { return false };
    let form = [
        ("token", record.refresh_token.as_str()),
        ("token_type_hint", "refresh_token"),
        ("client_id", record.client_id.as_str()),
    ];
    let Ok(client) = http_client() else { return false };
    match client.post(url).form(&form).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Form;
    use serde_json::json;

    use super::*;
    use crate::storage::Db;

    /// A form as the token endpoint got it.
    pub(crate) type Fields = Vec<(String, String)>;

    /// A token endpoint that renews with a new pair each time, or refuses as told; counts its renewals.
    #[derive(Clone, Default)]
    pub(crate) struct Issuer {
        pub renewals: Arc<AtomicUsize>,
        pub refuse: Arc<Mutex<Option<(u16, Value)>>>,
        pub forms: Arc<Mutex<Vec<Fields>>>,
        /// The nonce of the sign-in page it showed, for the ID token of the code's exchange.
        pub nonce: Arc<Mutex<Option<String>>>,
        /// What the code's exchange grants; the plan's scope with the rest when unset.
        pub scope: Arc<Mutex<Option<String>>>,
    }

    async fn token(State(issuer): State<Issuer>, Form(form): Form<Vec<(String, String)>>) -> axum::response::Response {
        issuer.forms.lock().unwrap().push(form.clone());
        if let Some((status, body)) = issuer.refuse.lock().unwrap().clone() {
            return (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response();
        }
        let field = |name: &str| form.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        if field("grant_type").as_deref() == Some("authorization_code") {
            if field("code").as_deref() != Some("good-code") {
                return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": "invalid_grant"}))).into_response();
            }
            let claims = json!({
                "iss": "https://issuer.example",
                "aud": field("client_id"),
                "nonce": issuer.nonce.lock().unwrap().clone(),
                "sub": "user-1",
                "email": "owner@example.com",
            });
            let scope = issuer.scope.lock().unwrap().clone().unwrap_or("openid email offline_access plan.use".into());
            return axum::Json(json!({
                "access_token": "access-code",
                "refresh_token": "refresh-code",
                "id_token": id_token(claims),
                "expires_in": 3600,
                "scope": scope,
                "token_type": "Bearer",
            }))
            .into_response();
        }
        let n = issuer.renewals.fetch_add(1, Ordering::SeqCst) + 1;
        // Slow enough for two askers to meet at the lock.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        axum::Json(json!({"access_token": format!("access-{n}"), "refresh_token": format!("refresh-{n}"), "expires_in": 3600, "token_type": "Bearer"}))
            .into_response()
    }

    pub(crate) async fn issuer() -> (Issuer, String) {
        let issuer = Issuer::default();
        let app = axum::Router::new().route("/token", axum::routing::post(token)).with_state(issuer.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (issuer, base)
    }

    pub(crate) fn flow(base: &str) -> &'static TokenFlow {
        Box::leak(Box::new(TokenFlow {
            authorize: Box::leak(format!("{base}/authorize").into_boxed_str()),
            token: Box::leak(format!("{base}/token").into_boxed_str()),
            revoke: None,
            issuer: "https://issuer.example",
            client: "dynamic_agent_client",
            registers: true,
            scope: "openid email offline_access plan.use",
            resource: Some("https://api.example/v1"),
            required_scope: Some("plan.use"),
            loopback: Some("/auth/callback"),
            host_id: true,
        }))
    }

    /// An unsigned ID token with these claims.
    pub(crate) fn id_token(claims: Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    fn access(base: &str, record: &TokenRecord) -> (Arc<TokenAccess>, Arc<SecretsService>) {
        let secrets = Arc::new(SecretsService::new(Arc::new(Db::memory()), None).unwrap());
        secrets.set("oauth:llm:chatgpt", Some(&serde_json::to_string(record).unwrap())).unwrap();
        (Arc::new(TokenAccess::new(secrets.clone(), "oauth:llm:chatgpt", flow(base), "ChatGPT")), secrets)
    }

    fn signed(expires_in_ms: i64) -> TokenRecord {
        TokenRecord {
            client_id: "oaiapp_1".into(),
            access_token: "access-0".into(),
            refresh_token: "refresh-0".into(),
            expires_at: now_ms() as i64 + expires_in_ms,
            email: Some("owner@example.com".into()),
            ..TokenRecord::default()
        }
    }

    #[test]
    fn reads_the_code_exchange_with_its_nonce_and_refuses_one_without_the_plan() {
        let flow = flow("http://unused");
        let token = id_token(
            json!({"iss": "https://issuer.example", "aud": "oaiapp_1", "nonce": "n1", "sub": "user-1", "email": "owner@example.com"}),
        );
        let answer = json!({"access_token": "a1", "refresh_token": "r1", "id_token": token, "expires_in": 3600, "scope": "openid plan.use"});
        let record = TokenRecord::issued(&answer, "oaiapp_1", "n1", flow, "ChatGPT").unwrap();
        assert_eq!((record.access_token.as_str(), record.refresh_token.as_str()), ("a1", "r1"));
        assert_eq!((record.email.as_deref(), record.subject.as_deref()), (Some("owner@example.com"), Some("user-1")));
        assert!(record.expires_at > now_ms() as i64 + 3500 * 1000);
        assert!(TokenRecord::issued(&answer, "oaiapp_1", "other-nonce", flow, "ChatGPT")
            .unwrap_err()
            .contains("чужой"));
        let without = json!({"access_token": "a1", "refresh_token": "r1", "scope": "openid email"});
        assert!(TokenRecord::issued(&without, "oaiapp_1", "n1", flow, "ChatGPT")
            .unwrap_err()
            .contains("без разрешения"));
        assert!(TokenRecord::issued(&json!({}), "oaiapp_1", "n1", flow, "ChatGPT").is_err());
        let later = json!({"access_token": "a", "earliest_refresh_at": 1_800_000_000});
        let mut timed = TokenRecord::default();
        timed.times(&later);
        assert_eq!(timed.earliest_refresh_at, 1_800_000_000_000, "seconds are read as seconds");
    }

    #[tokio::test]
    async fn gives_the_token_while_it_is_fresh_and_renews_it_once_for_everyone_waiting() {
        let (issuer, base) = issuer().await;
        let (access, _) = access(&base, &signed(30 * 60 * 1000));
        assert_eq!(access.bearer().await.unwrap(), "access-0");
        assert_eq!(issuer.renewals.load(Ordering::SeqCst), 0);

        let (access, secrets) = access_about_to_end(&base);
        let (one, two) = tokio::join!(access.bearer(), access.bearer());
        assert_eq!((one.unwrap(), two.unwrap()), ("access-1".to_string(), "access-1".to_string()));
        assert_eq!(issuer.renewals.load(Ordering::SeqCst), 1, "one renewal for both");
        let kept: TokenRecord = serde_json::from_str(&secrets.get("oauth:llm:chatgpt").unwrap()).unwrap();
        assert_eq!((kept.refresh_token.as_str(), kept.client_id.as_str()), ("refresh-1", "oaiapp_1"));
        let form = issuer.forms.lock().unwrap()[0].clone();
        let field = |name: &str| form.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        assert_eq!(field("grant_type"), Some("refresh_token"));
        assert_eq!((field("client_id"), field("refresh_token")), (Some("oaiapp_1"), Some("refresh-0")));
        assert_eq!(field("resource"), Some("https://api.example/v1"));
        assert_eq!(
            access.account(),
            AccountView { signed_in: true, who: Some("o…@example.com".into()), problem: None }
        );
    }

    fn access_about_to_end(base: &str) -> (Arc<TokenAccess>, Arc<SecretsService>) {
        access(base, &signed(30 * 1000))
    }

    #[tokio::test]
    async fn a_renewal_refused_for_good_ends_the_sign_in_and_a_network_failure_keeps_it() {
        let (issuer, base) = issuer().await;
        let (access, secrets) = access_about_to_end(&base);
        *issuer.refuse.lock().unwrap() = Some((503, json!({"error": "temporarily_unavailable"})));
        assert_eq!(access.bearer().await.unwrap(), "access-0", "the token still works for half a minute");
        assert!(access.record().unwrap().signed_in());

        *issuer.refuse.lock().unwrap() = Some((400, json!({"error": {"code": "refresh_token_reused"}})));
        let error = access.bearer().await.unwrap_err();
        let spoken = error.downcast_ref::<SpokenFailure>().expect("said in words");
        assert!(spoken.phrase.contains("Войдите снова"), "{}", spoken.phrase);
        let kept: TokenRecord = serde_json::from_str(&secrets.get("oauth:llm:chatgpt").unwrap()).unwrap();
        assert!(!kept.signed_in());
        assert_eq!(
            (kept.client_id.as_str(), kept.email.as_deref()),
            ("oaiapp_1", Some("owner@example.com")),
            "the registration stays"
        );
        assert_eq!(access.account().problem, Some(Problem::Signin));
        assert!(access.bearer().await.is_err());
    }

    #[tokio::test]
    async fn the_plans_limit_shows_until_a_request_goes_through() {
        let (_issuer, base) = issuer().await;
        let (access, _) = access(&base, &signed(30 * 60 * 1000));
        access.limited();
        assert_eq!(access.account().problem, Some(Problem::Limit));
        access.ok();
        assert_eq!(access.account().problem, None);
    }
}
