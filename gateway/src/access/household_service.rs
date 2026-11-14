//! Who speaks to the station. Nobody is recognised yet, so every voice is the household: the role and the
//! restrictions the admin gave it (a member by default — the station answered everyone before there were people).
//! This is the seam for recognition by name («это Аня») and, at home only, by voice: `speaker()` returns that
//! person's principal, and the router checks their rights the same way.

use std::sync::{Arc, RwLock};

use serde_json::{Map, Value};

use super::users_repository::clean_restrictions;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::principal::{household_principal, Principal, Restrictions};
use crate::core::identity::roles::Role;
use crate::platform::http::ApiError;
use crate::settings::definitions::{Household, HOUSEHOLD};
use crate::settings::SettingsService;

/// What a voice request knows about itself when the gateway asks who spoke.
#[derive(Debug, Clone, Default)]
pub struct SpeakerHint {
    pub device_tag: String,
    pub text: String,
}

struct Current {
    config: Household,
    principal: Principal,
}

pub struct HouseholdService {
    settings: Arc<SettingsService>,
    current: RwLock<Current>,
}

impl HouseholdService {
    /// The household's voice as stored. Until a read succeeds it is a guest — the least — never the member it is
    /// by default: a database that fails must not widen anyone's rights.
    pub fn new(settings: Arc<SettingsService>) -> HouseholdService {
        let config = Household { role: Role::Guest, restrictions: Restrictions::default() };
        let principal = household_principal(config.role, Some(&config.restrictions));
        let service = HouseholdService { settings, current: RwLock::new(Current { config, principal }) };
        service.refresh();
        service
    }

    /// Follows the setting when someone else changes it (another panel, a script).
    pub fn start(self: &Arc<Self>, events: &EventBus) {
        let household = Arc::downgrade(self);
        events.on("household", move |event| {
            if let GatewayEvent::SettingsChanged { key, user_id: None, .. } = &*event {
                if key == HOUSEHOLD.key {
                    if let Some(household) = household.upgrade() {
                        household.refresh();
                    }
                }
            }
        });
    }

    /// Synchronous: the router asks on every request.
    pub fn speaker(&self, _hint: Option<&SpeakerHint>) -> Principal {
        self.read().principal.clone()
    }

    pub fn view(&self) -> Household {
        self.read().config.clone()
    }

    pub fn set(&self, input: &Map<String, Value>, actor: Option<Actor>) -> Result<Household, ApiError> {
        let current = self.view();
        let role = match input.get("role") {
            None => Some(current.role),
            Some(value) => value.as_str().and_then(Role::parse),
        };
        let role = role
            .filter(|role| *role != Role::Admin)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("role: member или guest — голос дома не администратор")))?;
        let restrictions = match input.get("restrictions") {
            None => current.restrictions,
            Some(value) => clean_restrictions(Some(value)),
        };
        self.settings.set(&HOUSEHOLD, &Household { role, restrictions }, actor)?;
        self.refresh();
        Ok(self.view())
    }

    /// Reads the setting again. A read that fails keeps what was known (logged): not the default member.
    fn refresh(&self) {
        let stored = match self.settings.try_entry(&HOUSEHOLD) {
            Ok(entry) => entry.map(|entry| entry.value),
            Err(error) => {
                tracing::warn!("household not read, its voice stays as it was: {error}");
                return;
            }
        };
        let config = match stored {
            Some(stored) => Household {
                // The household's voice is never an admin, whatever a hand edit says.
                role: if stored.role == Role::Admin { Role::Member } else { stored.role },
                restrictions: clean_restrictions(serde_json::to_value(&stored.restrictions).ok().as_ref()),
            },
            None => Household { role: Role::Member, restrictions: Restrictions::default() },
        };
        let principal = household_principal(config.role, Some(&config.restrictions));
        *self.current.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Current { config, principal };
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Current> {
        self.current.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::can;
    use crate::storage::Db;
    use serde_json::json;

    fn service() -> (Arc<HouseholdService>, Arc<SettingsService>) {
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), EventBus::new()));
        (Arc::new(HouseholdService::new(settings.clone())), settings)
    }

    fn input(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn is_a_member_until_the_admin_says_otherwise() {
        let (household, _) = service();
        let voice = household.speaker(None);
        assert_eq!(voice.id, "household");
        assert!(can(Some(&voice), "use:calls"));
        let view = household.set(&input(json!({"role": "guest", "restrictions": {"deny": ["use:music"]}})), None);
        assert_eq!(view.unwrap().role, Role::Guest);
        let voice = household.speaker(None);
        assert!(!can(Some(&voice), "use:calls"));
        assert!(!can(Some(&voice), "use:music"));
        assert!(can(Some(&voice), "use:radio"));
        // Only the restrictions: the role stays.
        let view = household.set(&input(json!({"restrictions": {}})), None).unwrap();
        assert_eq!((view.role, view.restrictions), (Role::Guest, Restrictions::default()));
    }

    #[test]
    fn keeps_the_households_voice_when_its_setting_cannot_be_read_and_starts_closed() {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let household = HouseholdService::new(settings.clone());
        household.set(&input(json!({"role": "guest", "restrictions": {"deny": ["use:music"]}})), None).unwrap();
        db.with(|c| c.execute_batch("DROP TABLE settings")).unwrap();
        household.refresh();
        let voice = household.speaker(None);
        assert_eq!(voice.role, Some(Role::Guest), "not the default member");
        assert!(!can(Some(&voice), "use:music"), "the restriction stays");
        // A gateway that cannot read the setting at all starts with the least: a guest.
        let unread = HouseholdService::new(settings);
        assert_eq!(unread.view().role, Role::Guest);
        assert!(!can(Some(&unread.speaker(None)), "use:calls"));
    }

    #[test]
    fn the_households_voice_is_never_an_admin() {
        let (household, settings) = service();
        for role in [json!("admin"), json!("owner"), json!(null)] {
            let refused = household.set(&input(json!({ "role": role })), None).unwrap_err();
            assert_eq!(refused.message, "role: member или guest — голос дома не администратор");
        }
        settings
            .set(&HOUSEHOLD, &Household { role: Role::Admin, restrictions: Restrictions::default() }, None)
            .unwrap();
        household.refresh();
        assert_eq!(household.view().role, Role::Member);
    }

    #[tokio::test]
    async fn follows_a_change_made_elsewhere() {
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        let household = Arc::new(HouseholdService::new(settings.clone()));
        household.start(&events);
        settings
            .set(&HOUSEHOLD, &Household { role: Role::Guest, restrictions: Restrictions::default() }, None)
            .unwrap();
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(household.view().role, Role::Guest);
    }
}
