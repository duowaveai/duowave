//! The household's people, as the admin manages them. The rules that keep the house manageable live here, not in
//! the panel: names are unique (case aside), there is always an active admin who can sign in, and every change of a
//! person's password, role or status ends their sessions.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::js_len;
use super::passwords::{hash_password, password_problem, verify_password};
use super::users_repository::{
    clean_profile, clean_restrictions, person_view, PersonView, Profile, UserChanges, UserRecord, UserStatus, Users,
    UsersRepository,
};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::principal::{Principal, PrincipalKind, Restrictions};
use crate::core::identity::roles::Role;
use crate::platform::http::ApiError;
use crate::utils::ids::uuid;
use crate::utils::time::now_iso;

/// A person as the panel sends them: absent fields are not changed; `password: null` or `""` removes it.
pub type PersonInput = Map<String, Value>;

/// The events' actor of a principal, when there is one.
pub fn actor_of(principal: Option<&Principal>) -> Option<Actor> {
    principal.map(Actor::from)
}

const ADMIN_NEEDS_PASSWORD: &str = "Администратору нужен пароль: он входит в пульт.";
const NAME_TAKEN: &str = "Такое имя уже есть.";
const LAST_ADMIN: &str = "Это последний администратор: сначала назначьте другого.";
const NO_SUCH_PERSON: &str = "Нет такого человека.";

/// The longest name a person may have, in UTF-16 units as the panel counts.
pub const MAX_NAME: usize = 40;

pub struct UsersService {
    users: Arc<UsersRepository>,
    events: EventBus,
}

impl UsersService {
    pub fn new(users: Arc<UsersRepository>, events: EventBus) -> UsersService {
        UsersService { users, events }
    }

    pub fn list(&self) -> Result<Vec<PersonView>, ApiError> {
        Ok(self.users.list()?.iter().map(person_view).collect())
    }

    pub fn get(&self, id: &str) -> Result<PersonView, ApiError> {
        Ok(person_view(&self.found(id)?))
    }

    pub async fn create(&self, input: &PersonInput, actor: Option<Actor>) -> Result<PersonView, ApiError> {
        let name = valid_name(input.get("name"))?;
        if self.users.by_name(&name)?.is_some() {
            return Err(ApiError::bad_request(NAME_TAKEN));
        }
        let role = match input.get("role") {
            None => Role::Member,
            Some(value) => valid_role(value)?,
        };
        // A password is optional: without one the person is a profile the station knows, not a panel account.
        let password_hash = match input.get("password") {
            None => None,
            Some(Value::String(password)) if password.is_empty() => None,
            Some(password) => {
                if let Some(problem) = password_problem(Some(password)) {
                    return Err(ApiError::bad_request(problem));
                }
                Some(hash_password(password.as_str().unwrap_or_default()).await?)
            }
        };
        if role == Role::Admin && password_hash.is_none() {
            return Err(ApiError::bad_request(ADMIN_NEEDS_PASSWORD));
        }
        let now = now_iso();
        let user = UserRecord {
            id: uuid(),
            name,
            password_hash,
            role,
            status: UserStatus::Active,
            token_version: 0,
            restrictions: clean_restrictions(input.get("restrictions")),
            profile: clean_profile(input.get("profile")),
            last_login_at: None,
            created_at: now.clone(),
            updated_at: now,
        };
        // Checked again where it is written: «Аня» and «аня» added at once cannot both pass.
        self.users.transaction(|users| {
            if users.by_name(&user.name)?.is_some() {
                return Err(ApiError::bad_request(NAME_TAKEN));
            }
            Ok(users.insert(&user)?)
        })?;
        self.events.emit(GatewayEvent::UserCreated {
            user_id: user.id.clone(),
            name: user.name.clone(),
            role: user.role.as_str().into(),
            actor,
        });
        Ok(person_view(&user))
    }

    /// What is asked is checked first, in the Node gateway's order, and a new password hashed (it takes a while);
    /// then the person as they are now, the rules and the write go in one transaction: two admins demoting each other
    /// at once cannot leave the house without one.
    pub async fn update(&self, id: &str, input: &PersonInput, actor: Option<Actor>) -> Result<PersonView, ApiError> {
        self.found(id)?;
        let wanted = self.wanted(id, input).await?;
        let (next, changed) = self.users.transaction(|users| {
            let user = users.by_id(id)?.ok_or_else(|| ApiError::not_found(NO_SUCH_PERSON))?;
            let (changes, changed, end_sessions) = wanted.against(&user);
            if let Some(name) = &changes.name {
                if users.by_name(name)?.is_some_and(|other| other.id != id) {
                    return Err(ApiError::bad_request(NAME_TAKEN));
                }
            }
            let next = changes.applied_to(&user);
            if next.role == Role::Admin && next.password_hash.is_none() {
                return Err(ApiError::bad_request(ADMIN_NEEDS_PASSWORD));
            }
            keep_an_admin(users, &user, Some(&next))?;
            if !changed.is_empty() {
                users.update(id, &changes)?;
                if end_sessions {
                    users.bump_token_version(id)?;
                }
            }
            Ok((next, changed))
        })?;
        if !changed.is_empty() {
            self.events.emit(GatewayEvent::UserUpdated {
                user_id: id.to_string(),
                name: next.name.clone(),
                changes: changed,
                actor,
            });
        }
        Ok(person_view(&next))
    }

    /// An update's fields, each checked; the name not taken by someone else as things are now.
    async fn wanted(&self, id: &str, input: &PersonInput) -> Result<Wanted, ApiError> {
        let mut wanted = Wanted::default();
        if let Some(value) = input.get("name") {
            let name = valid_name(Some(value))?;
            if self.users.by_name(&name)?.is_some_and(|other| other.id != id) {
                return Err(ApiError::bad_request(NAME_TAKEN));
            }
            wanted.name = Some(name);
        }
        if let Some(value) = input.get("role") {
            wanted.role = Some(valid_role(value)?);
        }
        if let Some(value) = input.get("status") {
            wanted.status = Some(match value.as_str() {
                Some("active") => UserStatus::Active,
                Some("disabled") => UserStatus::Disabled,
                _ => return Err(ApiError::bad_request("status: active или disabled")),
            });
        }
        if let Some(value) = input.get("password") {
            wanted.password = Some(match value {
                Value::Null => None,
                Value::String(password) if password.is_empty() => None,
                password => {
                    if let Some(problem) = password_problem(Some(password)) {
                        return Err(ApiError::bad_request(problem));
                    }
                    Some(hash_password(password.as_str().unwrap_or_default()).await?)
                }
            });
        }
        if let Some(value) = input.get("restrictions") {
            wanted.restrictions = Some(clean_restrictions(Some(value)));
        }
        if let Some(value) = input.get("profile") {
            wanted.profile = Some(clean_profile(Some(value)));
        }
        Ok(wanted)
    }

    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let user = self.users.transaction(|users| {
            let user = users.by_id(id)?.ok_or_else(|| ApiError::not_found(NO_SUCH_PERSON))?;
            keep_an_admin(users, &user, None)?;
            users.remove(id)?;
            Ok(user)
        })?;
        self.events.emit(GatewayEvent::UserDeleted { user_id: id.to_string(), name: user.name, actor });
        Ok(())
    }

    /// «Выйти везде»: every token of the person stops working.
    pub fn sign_out_everywhere(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let user = self.found(id)?;
        self.users.bump_token_version(id)?;
        self.events.emit(GatewayEvent::UserUpdated {
            user_id: id.to_string(),
            name: user.name,
            changes: vec!["sessions".into()],
            actor,
        });
        Ok(())
    }

    /// One's own password: the current one first. Ends the other sessions; the caller gets a new token.
    pub async fn change_own_password(
        &self,
        principal: &Principal,
        current: Option<&Value>,
        next: Option<&Value>,
    ) -> Result<(), ApiError> {
        if principal.kind != PrincipalKind::User {
            return Err(ApiError::forbidden("Пароль меняет человек, а не ключ."));
        }
        let user = self.found(&principal.id)?;
        let right = match current.and_then(Value::as_str) {
            Some(current) => verify_password(current, user.password_hash.as_deref()).await,
            None => false,
        };
        if !right {
            return Err(ApiError::bad_request("Текущий пароль не тот."));
        }
        if let Some(problem) = password_problem(next) {
            return Err(ApiError::bad_request(problem));
        }
        let hash = hash_password(next.and_then(Value::as_str).unwrap_or_default()).await?;
        self.users.update(&user.id, &UserChanges { password_hash: Some(Some(hash)), ..UserChanges::default() })?;
        self.users.bump_token_version(&user.id)?;
        self.events.emit(GatewayEvent::UserUpdated {
            user_id: user.id,
            name: user.name,
            changes: vec!["password".into()],
            actor: Some(Actor::from(principal)),
        });
        Ok(())
    }

    fn found(&self, id: &str) -> Result<UserRecord, ApiError> {
        self.users.by_id(id)?.ok_or_else(|| ApiError::not_found(NO_SUCH_PERSON))
    }
}

/// The change must leave an active admin who can sign in; otherwise nobody could manage the house. Asked inside the
/// transaction that writes the change.
fn keep_an_admin(users: &Users<'_>, before: &UserRecord, after: Option<&UserRecord>) -> Result<(), ApiError> {
    if !before.active_admin() || after.is_some_and(UserRecord::active_admin) {
        return Ok(());
    }
    let others = users.list()?.into_iter().filter(|user| user.id != before.id && user.active_admin()).count();
    if others == 0 {
        return Err(ApiError::bad_request(LAST_ADMIN));
    }
    Ok(())
}

/// What an update asks, each field checked; None leaves it alone.
#[derive(Default)]
struct Wanted {
    name: Option<String>,
    role: Option<Role>,
    status: Option<UserStatus>,
    /// The new password's hash, or None to remove the password.
    password: Option<Option<String>>,
    restrictions: Option<Restrictions>,
    profile: Option<Profile>,
}

impl Wanted {
    /// Against the person as they are: the changes, what the event says changed, whether their sessions end.
    fn against(&self, user: &UserRecord) -> (UserChanges, Vec<String>, bool) {
        let mut changes = UserChanges::default();
        let mut changed: Vec<String> = Vec::new();
        let mut end_sessions = false;
        if let Some(name) = self.name.as_ref().filter(|name| **name != user.name) {
            changes.name = Some(name.clone());
            changed.push("name".into());
        }
        if let Some(role) = self.role.filter(|role| *role != user.role) {
            changes.role = Some(role);
            changed.push("role".into());
            end_sessions = true;
        }
        if let Some(status) = self.status.filter(|status| *status != user.status) {
            changes.status = Some(status);
            changed.push("status".into());
            end_sessions = true;
        }
        if let Some(password) = &self.password {
            changes.password_hash = Some(password.clone());
            changed.push("password".into());
            end_sessions = true;
        }
        if let Some(restrictions) = &self.restrictions {
            changes.restrictions = Some(restrictions.clone());
            changed.push("restrictions".into());
        }
        if let Some(profile) = &self.profile {
            changes.profile = Some(profile.clone());
            changed.push("profile".into());
        }
        (changes, changed, end_sessions)
    }
}

fn valid_name(value: Option<&Value>) -> Result<String, ApiError> {
    let name = value.and_then(Value::as_str).map(str::trim).unwrap_or("");
    if !(2..=MAX_NAME).contains(&js_len(name)) {
        return Err(ApiError::bad_request("Имя — от 2 до 40 символов."));
    }
    Ok(name.to_string())
}

fn valid_role(value: &Value) -> Result<Role, ApiError> {
    value.as_str().and_then(Role::parse).ok_or_else(|| ApiError::bad_request("role: admin, member или guest"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::users_repository::tests::person;
    use crate::storage::Db;
    use serde_json::json;

    fn service() -> (UsersService, Arc<UsersRepository>, tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) {
        let events = EventBus::new();
        let receiver = events.subscribe();
        let users = Arc::new(UsersRepository::new(Arc::new(Db::memory())));
        (UsersService::new(users.clone(), events), users, receiver)
    }

    fn input(value: Value) -> PersonInput {
        value.as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn adds_people_with_unique_names_and_profiles_without_a_password() {
        let (service, _, mut events) = service();
        let kid = service.create(&input(json!({"name": " Миша ", "role": "guest"})), None).await.unwrap();
        assert_eq!((kid.name.as_str(), kid.role, kid.can_sign_in), ("Миша", Role::Guest, false));
        assert!(matches!(&*events.try_recv().unwrap(), GatewayEvent::UserCreated { role, .. } if role == "guest"));
        let taken = service.create(&input(json!({"name": "миша"})), None).await.unwrap_err();
        assert_eq!(taken.message, NAME_TAKEN);
        let admin = service.create(&input(json!({"name": "Без пароля", "role": "admin"})), None).await.unwrap_err();
        assert_eq!(admin.message, ADMIN_NEEDS_PASSWORD);
        let short = service.create(&input(json!({"name": "Аня", "password": "short"})), None).await.unwrap_err();
        assert_eq!(short.message, "Пароль — хотя бы 8 символов.");
        assert_eq!(
            service.create(&input(json!({"name": "А"})), None).await.unwrap_err().message,
            "Имя — от 2 до 40 символов."
        );
        assert_eq!(
            service.create(&input(json!({"name": "Аня", "role": "owner"})), None).await.unwrap_err().message,
            "role: admin, member или guest"
        );
    }

    #[tokio::test]
    async fn ends_sessions_when_rights_change_but_not_for_a_profile_edit() {
        let (service, users, _) = service();
        users.insert(&person("a", "admin", Role::Admin, Some("scrypt:x:y"))).unwrap();
        users.insert(&person("u", "Аня", Role::Member, Some("scrypt:x:y"))).unwrap();
        service.update("u", &input(json!({"profile": {"color": "#112233"}})), None).await.unwrap();
        assert_eq!(users.by_id("u").unwrap().unwrap().token_version, 0);
        let view = service
            .update(
                "u",
                &input(json!({"role": "guest", "restrictions": {"deny": ["use:music", "use:nonsense"]}})),
                None,
            )
            .await
            .unwrap();
        assert_eq!(view.role, Role::Guest);
        assert_eq!(view.restrictions.deny, Some(vec!["use:music".to_string()]));
        assert_eq!(users.by_id("u").unwrap().unwrap().token_version, 1);
        service.update("u", &input(json!({"password": null})), None).await.unwrap();
        let after = users.by_id("u").unwrap().unwrap();
        assert_eq!((after.password_hash, after.token_version), (None, 2));
        assert_eq!(
            service.update("u", &input(json!({"status": "gone"})), None).await.unwrap_err().message,
            "status: active или disabled"
        );
        assert_eq!(
            service.update("nobody", &input(json!({})), None).await.unwrap_err().message,
            "Нет такого человека."
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_admins_demoting_each_other_at_once_leave_one_of_them() {
        for round in 0..25 {
            let (service, users, _) = service();
            let service = Arc::new(service);
            users.insert(&person("a", "Первый", Role::Admin, Some("scrypt:x:y"))).unwrap();
            users.insert(&person("b", "Второй", Role::Admin, Some("scrypt:x:y"))).unwrap();
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let demote = |id: &'static str| {
                let (service, barrier) = (service.clone(), barrier.clone());
                tokio::spawn(async move {
                    barrier.wait().await;
                    service.update(id, &input(json!({"role": "member"})), None).await
                })
            };
            let (first, second) = (demote("a"), demote("b"));
            let results = [first.await.unwrap(), second.await.unwrap()];
            let refused = results.iter().filter(|result| result.as_ref().is_err_and(|e| e.message == LAST_ADMIN));
            assert_eq!(refused.count(), 1, "round {round}: {results:?}");
            let admins = users.list().unwrap().into_iter().filter(UserRecord::active_admin).count();
            assert_eq!(admins, 1, "round {round}");
        }
    }

    #[tokio::test]
    async fn keeps_an_active_admin_who_can_sign_in() {
        let (service, users, _) = service();
        users.insert(&person("a", "admin", Role::Admin, Some("scrypt:x:y"))).unwrap();
        let last = "Это последний администратор: сначала назначьте другого.";
        assert_eq!(service.update("a", &input(json!({"role": "member"})), None).await.unwrap_err().message, last);
        assert_eq!(service.update("a", &input(json!({"status": "disabled"})), None).await.unwrap_err().message, last);
        assert_eq!(
            service.update("a", &input(json!({"password": ""})), None).await.unwrap_err().message,
            ADMIN_NEEDS_PASSWORD
        );
        assert_eq!(service.remove("a", None).unwrap_err().message, last);
        users.insert(&person("b", "второй", Role::Admin, Some("scrypt:x:y"))).unwrap();
        service.update("a", &input(json!({"role": "member"})), None).await.unwrap();
        assert_eq!(service.remove("b", None).unwrap_err().message, last, "now b is the last one");
        service.remove("a", None).unwrap();
    }

    #[tokio::test]
    async fn changes_ones_own_password_only_for_a_person_who_knows_the_current_one() {
        let (service, users, _) = service();
        let hash = hash_password("current-password").await.unwrap();
        users.insert(&person("u", "Аня", Role::Member, Some(&hash))).unwrap();
        let anya = crate::core::identity::principal::user_principal("u", "Аня", Role::Member, None);
        let wrong = service.change_own_password(&anya, Some(&json!("wrong")), Some(&json!("next-password"))).await;
        assert_eq!(wrong.unwrap_err().message, "Текущий пароль не тот.");
        let short = service.change_own_password(&anya, Some(&json!("current-password")), Some(&json!("short"))).await;
        assert_eq!(short.unwrap_err().message, "Пароль — хотя бы 8 символов.");
        service
            .change_own_password(&anya, Some(&json!("current-password")), Some(&json!("next-password")))
            .await
            .unwrap();
        let after = users.by_id("u").unwrap().unwrap();
        assert_eq!(after.token_version, 1);
        assert!(verify_password("next-password", after.password_hash.as_deref()).await);
        let key = crate::core::identity::principal::api_key_principal("k", "script", &["*".into()]);
        let refused = service.change_own_password(&key, Some(&json!("x")), Some(&json!("y"))).await;
        assert_eq!(refused.unwrap_err().status, axum::http::StatusCode::FORBIDDEN);
    }
}
