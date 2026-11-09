//! Access: people, sign-in, tokens, API keys, the household's voice, the audit log (src/core/identity holds the rules
//! they follow). Every module checks rights through the same services; the gate in front of every HTTP route is the
//! `Caller` (platform/http/caller.rs), which asks `authenticate` who signed a request.

pub mod api_keys_service;
pub mod audit_service;
pub mod auth_service;
pub mod controller;
pub mod household_service;
pub mod passwords;
pub mod throttle;
pub mod tokens;
pub mod users_repository;
pub mod users_service;

use std::sync::Arc;

pub use api_keys_service::ApiKeysService;
pub use audit_service::AuditService;
pub use auth_service::AuthService;
pub use controller::routes;
pub use household_service::{HouseholdService, SpeakerHint};
pub use users_service::{actor_of, UsersService};

use crate::core::events::EventBus;
use crate::core::identity::Principal;
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::storage::Db;
use users_repository::UsersRepository;

pub struct AccessService {
    pub users: Arc<UsersService>,
    pub keys: Arc<ApiKeysService>,
    pub auth: Arc<AuthService>,
    pub household: Arc<HouseholdService>,
    pub audit: Arc<AuditService>,
    events: EventBus,
}

impl AccessService {
    /// The services, with the signing secret made at the first start (sealed among the secrets) and, while nobody
    /// exists, the first-run code in the log.
    pub fn new(
        db: Arc<Db>,
        events: EventBus,
        settings: Arc<SettingsService>,
        secrets: &SecretsService,
    ) -> anyhow::Result<AccessService> {
        let repository = Arc::new(UsersRepository::new(db.clone()));
        let keys = Arc::new(ApiKeysService::new(db.clone(), events.clone()));
        let auth = Arc::new(AuthService::new(repository.clone(), keys.clone(), events.clone(), secrets)?);
        Ok(AccessService {
            users: Arc::new(UsersService::new(repository, events.clone())),
            keys,
            auth,
            household: Arc::new(HouseholdService::new(settings)),
            audit: Arc::new(AuditService::new(db)),
            events,
        })
    }

    /// The audit log and the household's voice start following the bus (inside a tokio runtime).
    pub fn start(&self) {
        self.audit.start(&self.events);
        self.household.start(&self.events);
    }

    /// The principal of an `Authorization: Bearer …` header: a panel token or an API key. A sign-in that could not be
    /// checked (the database failed) reads as nobody here; `try_authenticate` tells it apart.
    pub fn authenticate(&self, authorization: Option<&str>) -> Option<Principal> {
        self.auth.authenticate(authorization)
    }

    /// The same, with a failed check as the server's error rather than «not signed in».
    pub fn try_authenticate(&self, authorization: Option<&str>) -> Result<Option<Principal>, crate::ApiError> {
        self.auth.try_authenticate(authorization)
    }
}

/// A text's length as JavaScript counts it (UTF-16 units): the panel's limits are the Node gateway's.
pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The start of a text up to `max` UTF-16 units, never splitting a character.
pub(crate) fn js_prefix(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_cuts_text_as_javascript_did() {
        assert_eq!(js_len("Аня"), 3);
        assert_eq!(js_len("🙂"), 2);
        assert_eq!(js_prefix("Анна-Мария", 4), "Анна");
        assert_eq!(js_prefix("a🙂b", 2), "a", "a character is never split");
        assert_eq!(js_prefix("abc", 40), "abc");
    }
}
