//! Signing in and telling who signed a request. People sign in with a password and get a token for a month; a key
//! (ApiKeysService) is a token of its own. The first password ever set makes the admin; who else exists and what they
//! may do is the UsersService's.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use subtle::ConstantTimeEq;

use super::api_keys_service::ApiKeysService;
use super::js_prefix;
use super::passwords::{hash_password, verify_password};
use super::throttle::LoginThrottle;
use super::tokens::{issue_token, read_token, TokenClaims};
use super::users_repository::{person_view, PersonView, Profile, UserChanges, UserRecord, UserStatus, UsersRepository};
use super::users_service::MAX_NAME;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::principal::{user_principal, Principal, Restrictions};
use crate::core::identity::roles::Role;
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::utils::ids::{random_hex, uuid};
use crate::utils::time::{now_iso, now_ms};

/// Where the panel's signing secret is sealed (src/secrets): the database and its copies hold only ciphertext.
pub const PANEL_SECRET: &str = "panel:secret";

/// A session's lifetime: a home tool, remembered for a month per device.
const TOKEN_DAYS: u64 = 30;
const DAY_MS: u64 = 86_400_000;

/// A person signed in: the token and who they are.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignedIn {
    pub token: String,
    pub user: PersonView,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginResult {
    Ok(Box<SignedIn>),
    Wrong,
    Throttled { wait_ms: u64 },
}

/// Why the first run from another device is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRefused {
    WrongCode,
    Throttled { wait_ms: u64 },
}

pub struct AuthService {
    users: Arc<UsersRepository>,
    keys: Arc<ApiKeysService>,
    events: EventBus,
    throttle: LoginThrottle,
    secret: String,
    /// Checked when the name is unknown, so a wrong name takes as long as a wrong password. Its digest is random:
    /// nothing ever matches it, only the time to find that out counts.
    dummy_hash: String,
    /// The first run from another device (a phone on the Wi-Fi) needs this code from the gateway's log: whoever is on
    /// the home network first must not become the admin. From this computer no code is asked.
    setup_code: Mutex<String>,
}

impl AuthService {
    pub fn new(
        users: Arc<UsersRepository>,
        keys: Arc<ApiKeysService>,
        events: EventBus,
        secrets: &SecretsService,
    ) -> Result<AuthService, ApiError> {
        let secret = signing_secret(secrets)?;
        let configured = users.count()? > 0;
        let setup_code = if configured {
            String::new()
        } else {
            let code = format!("{}-{}", rand::random_range(1000..10000), rand::random_range(1000..10000));
            tracing::warn!("первая настройка пульта: с этого компьютера — без кода, с другого устройства — код {code}");
            code
        };
        Ok(AuthService {
            users,
            keys,
            events,
            throttle: LoginThrottle::default(),
            secret,
            dummy_hash: format!("scrypt:{}:{}", random_hex(16), random_hex(32)),
            setup_code: Mutex::new(setup_code),
        })
    }

    /// Whether the first-run code is right (from this computer none is needed).
    pub fn setup_code_matches(&self, code: Option<&Value>) -> bool {
        let expected = self.setup_code.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let Some(given) = code.and_then(Value::as_str).map(str::trim) else { return false };
        !expected.is_empty() && given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
    }

    /// The first-run code from another device, counted per source as a password is: it has eight digits, and whoever
    /// is on the Wi-Fi must not try them all. A right code clears the source's count.
    pub fn check_setup_code(&self, code: Option<&Value>, source: &str) -> Result<(), SetupRefused> {
        self.check_setup_code_at(code, source, now_ms())
    }

    fn check_setup_code_at(&self, code: Option<&Value>, source: &str, now: u64) -> Result<(), SetupRefused> {
        let wait_ms = self.throttle.attempt(source, now);
        if wait_ms > 0 {
            return Err(SetupRefused::Throttled { wait_ms });
        }
        if !self.setup_code_matches(code) {
            return Err(SetupRefused::WrongCode);
        }
        self.throttle.succeed(source);
        Ok(())
    }

    /// Whether the first-run wizard is done: somebody exists.
    pub fn configured(&self) -> Result<bool, ApiError> {
        Ok(self.users.count()? > 0)
    }

    /// People who sign in: the login screen asks for a name only from the second one on.
    pub fn accounts(&self) -> Result<usize, ApiError> {
        Ok(self.users.list()?.iter().filter(|user| user.signs_in()).count())
    }

    /// The wizard's step: the first password makes the admin. None when somebody exists already.
    pub async fn setup(&self, password: &str, name: &str, source: &str) -> Result<Option<SignedIn>, ApiError> {
        let now = now_iso();
        let name = name.trim();
        let admin = UserRecord {
            id: uuid(),
            name: if name.is_empty() { "admin".into() } else { name.to_string() },
            password_hash: Some(hash_password(password).await?),
            role: Role::Admin,
            status: UserStatus::Active,
            token_version: 0,
            restrictions: Restrictions::default(),
            profile: Profile::default(),
            last_login_at: Some(now.clone()),
            created_at: now.clone(),
            updated_at: now,
        };
        if !self.users.insert_if_empty(&admin)? {
            return Ok(None);
        }
        self.setup_code.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        self.events.emit(GatewayEvent::UserCreated {
            user_id: admin.id.clone(),
            name: admin.name.clone(),
            role: admin.role.as_str().into(),
            actor: None,
        });
        self.events.emit(GatewayEvent::AuthLogin {
            user_id: admin.id.clone(),
            name: admin.name.clone(),
            source: source.to_string(),
        });
        Ok(Some(SignedIn { token: self.issue(&admin), user: person_view(&admin) }))
    }

    /// The name may be left out while exactly one person signs in. The check runs in a task of its own: a client that
    /// goes away mid-hash does not stop it, so its try stays counted and its failure is heard.
    pub async fn login(
        self: &Arc<Self>,
        name: Option<&str>,
        password: &str,
        source: &str,
    ) -> Result<LoginResult, ApiError> {
        let auth = self.clone();
        let (name, password, source) = (name.map(str::to_string), password.to_string(), source.to_string());
        tokio::spawn(async move { auth.login_at(name.as_deref(), &password, &source, now_ms()).await })
            .await
            .map_err(ApiError::internal)?
    }

    async fn login_at(
        &self,
        name: Option<&str>,
        password: &str,
        source: &str,
        now: u64,
    ) -> Result<LoginResult, ApiError> {
        // The try counts from here, before the hash: tries in parallel cannot pass the limit together.
        let wait_ms = self.throttle.attempt(source, now);
        if wait_ms > 0 {
            return Ok(LoginResult::Throttled { wait_ms });
        }
        let named = name.map(str::trim).filter(|name| !name.is_empty());
        let who = match named {
            Some(name) => self.users.by_name(name)?,
            None => {
                let mut people: Vec<UserRecord> = self.users.list()?.into_iter().filter(UserRecord::signs_in).collect();
                if people.len() == 1 {
                    people.pop()
                } else {
                    None
                }
            }
        };
        // Whoever cannot sign in costs the same hash as a wrong password.
        let ok = match who.as_ref().filter(|user| user.signs_in()) {
            Some(user) => verify_password(password, user.password_hash.as_deref()).await,
            None => {
                let _ = verify_password(password, Some(&self.dummy_hash)).await;
                false
            }
        };
        let Some(who) = who.filter(|_| ok) else {
            // Counted already; the name goes to the audit log no longer than a person's name can be.
            let name = named.map(|name| js_prefix(name, MAX_NAME).to_string());
            self.events.emit(GatewayEvent::AuthFailed { name, source: source.to_string() });
            return Ok(LoginResult::Wrong);
        };
        self.throttle.succeed(source);
        let signed_in = now_iso();
        self.users
            .update(&who.id, &UserChanges { last_login_at: Some(signed_in.clone()), ..UserChanges::default() })?;
        self.events.emit(GatewayEvent::AuthLogin {
            user_id: who.id.clone(),
            name: who.name.clone(),
            source: source.to_string(),
        });
        let user = UserRecord { last_login_at: Some(signed_in), ..who };
        Ok(LoginResult::Ok(Box::new(SignedIn { token: self.issue(&user), user: person_view(&user) })))
    }

    /// Who signed a request: a person's token or a key; None for anything else. A database that fails is an error,
    /// not «nobody»: the panel must not take it for a session that ended.
    pub fn try_authenticate(&self, authorization: Option<&str>) -> Result<Option<Principal>, ApiError> {
        let Some(token) = authorization.and_then(|header| header.strip_prefix("Bearer ")).map(str::trim) else {
            return Ok(None);
        };
        if token.is_empty() {
            return Ok(None);
        }
        if ApiKeysService::looks_like_key(token) {
            return self.keys.try_verify(token);
        }
        let Some(claims) = read_token(token, &self.secret, now_ms()) else { return Ok(None) };
        let Some(user) = self.users.by_id(&claims.user_id)? else { return Ok(None) };
        if !user.signs_in() || user.token_version != claims.version {
            return Ok(None);
        }
        Ok(Some(user_principal(&user.id, &user.name, user.role, Some(&user.restrictions))))
    }

    /// The same, a failed check read as nobody (it is logged): for whoever only asks whether anyone signed.
    pub fn authenticate(&self, authorization: Option<&str>) -> Option<Principal> {
        self.try_authenticate(authorization).unwrap_or(None)
    }

    /// A fresh token for a person, after a password change made the old ones stale.
    pub fn reissue(&self, user_id: &str) -> Result<Option<String>, ApiError> {
        Ok(self.users.by_id(user_id)?.filter(|user| user.status == UserStatus::Active).map(|user| self.issue(&user)))
    }

    fn issue(&self, user: &UserRecord) -> String {
        let claims = TokenClaims {
            user_id: user.id.clone(),
            version: user.token_version,
            expires_at: now_ms() + TOKEN_DAYS * DAY_MS,
        };
        issue_token(&claims, &self.secret)
    }

    #[cfg(test)]
    pub(crate) fn throttle(&self) -> &LoginThrottle {
        &self.throttle
    }

    /// The code the log shows: tests read it here.
    #[cfg(test)]
    pub(crate) fn setup_code(&self) -> String {
        self.setup_code.lock().unwrap().clone()
    }
}

/// The secret the panel's tokens are signed with, sealed among the secrets and made at the first start. When it does
/// not open (the key file was lost), a new one is made and everyone signs in again.
fn signing_secret(secrets: &SecretsService) -> Result<String, ApiError> {
    let sealed = secrets.get(PANEL_SECRET);
    if let Some(secret) = sealed.as_ref().filter(|secret| secret.len() >= 32) {
        return Ok(secret.clone());
    }
    if sealed.is_some() {
        tracing::warn!("the token secret was not usable and was replaced: everyone signs in again");
    }
    let secret = random_hex(32);
    secrets.set(PANEL_SECRET, Some(&secret))?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::users_repository::tests::person;
    use crate::storage::Db;
    use std::time::Duration;

    struct Built {
        auth: Arc<AuthService>,
        users: Arc<UsersRepository>,
        secrets: SecretsService,
        db: Arc<Db>,
        events: tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>,
    }

    impl Built {
        /// The failed sign-ins published so far: the name and the source.
        fn failures(&mut self) -> Vec<(Option<String>, String)> {
            let mut failed = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                if let GatewayEvent::AuthFailed { name, source } = &*event {
                    failed.push((name.clone(), source.clone()));
                }
            }
            failed
        }
    }

    fn build() -> Built {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let receiver = events.subscribe();
        let users = Arc::new(UsersRepository::new(db.clone()));
        let keys = Arc::new(ApiKeysService::new(db.clone(), events.clone()));
        let secrets = SecretsService::new(db.clone(), None).unwrap();
        let auth = Arc::new(AuthService::new(users.clone(), keys, events, &secrets).unwrap());
        Built { auth, users, secrets, db, events: receiver }
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    #[tokio::test]
    async fn the_first_password_makes_the_admin_once() {
        let mut built = build();
        assert!(!built.auth.configured().unwrap());
        let first = built.auth.setup("admin-password-1", "  ", "setup").await.unwrap().unwrap();
        assert_eq!((first.user.name.as_str(), first.user.role, first.user.can_sign_in), ("admin", Role::Admin, true));
        assert!(built.auth.configured().unwrap());
        assert_eq!(built.auth.accounts().unwrap(), 1);
        assert!(built.auth.setup("another-password", "Второй", "setup").await.unwrap().is_none());
        assert_eq!(built.users.count().unwrap(), 1);
        let names: Vec<&str> = [built.events.try_recv().unwrap(), built.events.try_recv().unwrap()]
            .iter()
            .map(|event| event.name())
            .collect();
        assert_eq!(names, ["user.created", "auth.login"]);
        let principal = built.auth.authenticate(Some(&bearer(&first.token))).unwrap();
        assert_eq!((principal.id, principal.role), (first.user.id, Some(Role::Admin)));
    }

    #[tokio::test]
    async fn asks_for_the_setup_code_from_another_device_until_someone_exists() {
        let built = build();
        let code = built.auth.setup_code();
        assert_eq!(code.len(), 9, "NNNN-NNNN: {code}");
        assert!(built.auth.setup_code_matches(Some(&Value::String(format!(" {code} ")))));
        assert!(!built.auth.setup_code_matches(Some(&Value::String("0000-0000".into()))));
        assert!(!built.auth.setup_code_matches(None));
        built.auth.setup("admin-password-1", "admin", "setup").await.unwrap();
        assert!(!built.auth.setup_code_matches(Some(&Value::String(code))), "used up");
    }

    #[tokio::test]
    async fn signs_in_by_name_or_alone_and_counts_failures_per_source() {
        let mut built = build();
        let hash = hash_password("anya-password").await.unwrap();
        built.users.insert(&person("u1", "Аня", Role::Member, Some(&hash))).unwrap();
        let LoginResult::Ok(alone) = built.auth.login(None, "anya-password", "10.0.0.5").await.unwrap() else {
            panic!("the only person signs in without a name");
        };
        assert!(alone.user.last_login_at.is_some());
        built.users.insert(&person("u2", "Миша", Role::Guest, None)).unwrap();
        built.users.insert(&person("u3", "Папа", Role::Member, Some(&hash))).unwrap();
        assert_eq!(built.auth.login(None, "anya-password", "10.0.0.5").await.unwrap(), LoginResult::Wrong);
        assert!(matches!(built.auth.login(Some(" аня "), "anya-password", "x").await.unwrap(), LoginResult::Ok(_)));
        assert_eq!(built.auth.login(Some("Миша"), "anything", "x").await.unwrap(), LoginResult::Wrong);
        assert_eq!(built.failures(), [(None, "10.0.0.5".to_string()), (Some("Миша".to_string()), "x".to_string())]);
        for _ in 0..5 {
            built.auth.login_at(Some("Аня"), "wrong", "attacker", 1000).await.unwrap();
        }
        let LoginResult::Throttled { wait_ms } =
            built.auth.login_at(Some("Аня"), "anya-password", "attacker", 2000).await.unwrap()
        else {
            panic!("throttled");
        };
        assert_eq!(wait_ms, 15 * 60_000 - 1000);
        assert_eq!(built.auth.throttle().wait_ms("10.0.0.5", now_ms()), 0, "another source signs in");
    }

    #[tokio::test]
    async fn checks_no_more_than_five_passwords_however_many_tries_come_at_once() {
        let mut built = build();
        let hash = hash_password("anya-password").await.unwrap();
        built.users.insert(&person("u1", "Аня", Role::Member, Some(&hash))).unwrap();
        let tries =
            (0..20).map(|n| built.auth.login(Some("Аня"), if n == 19 { "anya-password" } else { "wrong" }, "x"));
        let results = futures::future::join_all(tries).await;
        let wrong = results.iter().filter(|result| matches!(result, Ok(LoginResult::Wrong))).count();
        let throttled = results.iter().filter(|result| matches!(result, Ok(LoginResult::Throttled { .. }))).count();
        assert_eq!((wrong, throttled), (5, 15), "the right password at the end waits too");
        assert_eq!(built.failures().len(), 5);
    }

    #[tokio::test]
    async fn a_client_that_goes_away_mid_hash_has_still_used_its_try_and_is_heard() {
        let mut built = build();
        let hash = hash_password("anya-password").await.unwrap();
        built.users.insert(&person("u1", "Аня", Role::Member, Some(&hash))).unwrap();
        for _ in 0..5 {
            let gone =
                tokio::time::timeout(Duration::from_millis(1), built.auth.login(Some("Аня"), "wrong", "x")).await;
            assert!(gone.is_err(), "the hash takes longer than the client waits");
        }
        let heard = || async {
            for _ in 0..200 {
                if built.auth.throttle().wait_ms("x", now_ms()) > 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            false
        };
        assert!(heard().await, "five tries counted");
        assert!(matches!(
            built.auth.login(Some("Аня"), "anya-password", "x").await.unwrap(),
            LoginResult::Throttled { .. }
        ));
        let mut failed = Vec::new();
        for _ in 0..200 {
            failed.extend(built.failures());
            if failed.len() == 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(failed.len(), 5, "each failure reaches the audit log");
    }

    #[tokio::test]
    async fn writes_no_more_of_a_wrong_name_than_a_person_may_have() {
        let mut built = build();
        let long = "Я".repeat(10_000);
        assert_eq!(built.auth.login(Some(&long), "wrong-password", "x").await.unwrap(), LoginResult::Wrong);
        let failed = built.failures();
        assert_eq!(failed[0].0.as_deref(), Some("Я".repeat(MAX_NAME).as_str()));
    }

    #[test]
    fn counts_wrong_setup_codes_per_source_like_passwords() {
        let built = build();
        let code = Value::String(built.auth.setup_code());
        let wrong = Value::String("0000-0000".into());
        for _ in 0..4 {
            assert_eq!(built.auth.check_setup_code_at(Some(&wrong), "phone", 1000), Err(SetupRefused::WrongCode));
        }
        assert_eq!(built.auth.check_setup_code_at(Some(&code), "phone", 1000), Ok(()), "the right code clears");
        for _ in 0..5 {
            assert_eq!(built.auth.check_setup_code_at(None, "phone", 2000), Err(SetupRefused::WrongCode));
        }
        assert_eq!(
            built.auth.check_setup_code_at(Some(&code), "phone", 3000),
            Err(SetupRefused::Throttled { wait_ms: 15 * 60_000 - 1000 })
        );
        assert_eq!(built.auth.check_setup_code_at(Some(&code), "laptop", 3000), Ok(()), "per source");
    }

    #[tokio::test]
    async fn a_database_that_fails_is_an_error_not_nobody() {
        let built = build();
        let first = built.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
        let header = bearer(&first.token);
        built.db.with(|c| c.execute_batch("DROP TABLE user_settings; DROP TABLE users; DROP TABLE api_keys;")).unwrap();
        let error = built.auth.try_authenticate(Some(&header)).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        let key = format!("Bearer vgw_0123456789ab_{}", "A".repeat(43));
        assert!(built.auth.try_authenticate(Some(&key)).is_err(), "a key's check too");
        assert!(built.auth.authenticate(Some(&header)).is_none(), "the lenient form reads it as nobody");
        assert_eq!(built.auth.try_authenticate(Some("Bearer junk")).unwrap(), None, "a bad token is still nobody");
    }

    #[tokio::test]
    async fn a_token_ends_with_its_version_status_or_password() {
        let built = build();
        let first = built.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
        let header = bearer(&first.token);
        assert!(built.auth.authenticate(Some(&header)).is_some());
        assert!(built.auth.authenticate(Some(&first.token)).is_none(), "Bearer is required");
        assert!(built.auth.authenticate(Some("Bearer ")).is_none());
        assert!(built.auth.authenticate(None).is_none());
        built.users.bump_token_version(&first.user.id).unwrap();
        assert!(built.auth.authenticate(Some(&header)).is_none());
        let fresh = built.auth.reissue(&first.user.id).unwrap().unwrap();
        assert!(built.auth.authenticate(Some(&bearer(&fresh))).is_some());
        built
            .users
            .update(&first.user.id, &UserChanges { status: Some(UserStatus::Disabled), ..UserChanges::default() })
            .unwrap();
        assert!(built.auth.authenticate(Some(&bearer(&fresh))).is_none());
        assert!(built.auth.reissue(&first.user.id).unwrap().is_none());
    }

    #[test]
    fn keeps_the_signing_secret_sealed_across_a_restart() {
        let built = build();
        let stored: String = built
            .db
            .with(|c| c.query_row("SELECT ciphertext FROM secrets WHERE id = ?1", [PANEL_SECRET], |row| row.get(0)))
            .unwrap();
        assert!(!stored.contains(&built.auth.secret));
        let again = signing_secret(&built.secrets).unwrap();
        assert_eq!(again, built.auth.secret);
        built.secrets.set(PANEL_SECRET, Some("too-short")).unwrap();
        let replaced = signing_secret(&built.secrets).unwrap();
        assert_eq!(replaced.len(), 64);
        assert_ne!(replaced, built.auth.secret);
    }
}
