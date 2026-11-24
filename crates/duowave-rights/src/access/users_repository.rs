//! The `users` table: the household's people as the gateway keeps them, and as the panel sees them. The hash never
//! leaves the access module.

use std::sync::Arc;

use rusqlite::types::ToSql;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::identity::permissions::is_grant;
use crate::core::identity::principal::Restrictions;
use crate::core::identity::roles::{normalize_role, Role};
use crate::platform::http::ApiError;
use crate::storage::Db;
use crate::utils::time::now_iso;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The person's colour in the panel (DESIGN.md: everyone's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Names the person is called by: how the station will tell who speaks («это Аня»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Disabled,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UserStatus::Active => "active",
            UserStatus::Disabled => "disabled",
        }
    }
}

/// A person as the gateway keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct UserRecord {
    pub id: String,
    pub name: String,
    pub password_hash: Option<String>,
    pub role: Role,
    pub status: UserStatus,
    pub token_version: u64,
    pub restrictions: Restrictions,
    pub profile: Profile,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl UserRecord {
    /// Active and with a password: signs in to the panel.
    pub fn signs_in(&self) -> bool {
        self.status == UserStatus::Active && self.password_hash.is_some()
    }

    /// An admin who can sign in: the house stays manageable while one exists.
    pub fn active_admin(&self) -> bool {
        self.role == Role::Admin && self.signs_in()
    }
}

/// A person as the panel sees them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonView {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub status: UserStatus,
    /// Has a password: signs in to the panel. Without one — a profile the station knows.
    pub can_sign_in: bool,
    pub restrictions: Restrictions,
    pub profile: Profile,
    pub last_login_at: Option<String>,
    pub created_at: String,
}

pub fn person_view(record: &UserRecord) -> PersonView {
    PersonView {
        id: record.id.clone(),
        name: record.name.clone(),
        role: record.role,
        status: record.status,
        can_sign_in: record.password_hash.is_some(),
        restrictions: record.restrictions.clone(),
        profile: record.profile.clone(),
        last_login_at: record.last_login_at.clone(),
        created_at: record.created_at.clone(),
    }
}

/// Only grants the catalog knows survive a read: a stored typo never widens anyone's rights.
pub fn clean_restrictions(value: Option<&Value>) -> Restrictions {
    let object = value.and_then(Value::as_object);
    let list = |name: &str| -> Option<Vec<String>> {
        let items = object?.get(name)?.as_array()?;
        let mut grants: Vec<String> = Vec::new();
        for grant in items.iter().filter_map(Value::as_str).filter(|grant| is_grant(grant)) {
            if !grants.iter().any(|known| known == grant) {
                grants.push(grant.to_string());
            }
        }
        (!grants.is_empty()).then_some(grants)
    };
    Restrictions { allow: list("allow"), deny: list("deny") }
}

pub fn clean_profile(value: Option<&Value>) -> Profile {
    let object = value.and_then(Value::as_object);
    let color = object
        .and_then(|o| o.get("color"))
        .and_then(Value::as_str)
        .filter(|color| color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string);
    let aliases = object.and_then(|o| o.get("aliases")).and_then(Value::as_array).map(|items| {
        let mut aliases: Vec<String> = Vec::new();
        for alias in items.iter().filter_map(Value::as_str).map(str::trim).filter(|alias| !alias.is_empty()) {
            if !aliases.iter().any(|known| known == alias) {
                aliases.push(alias.to_string());
            }
        }
        aliases.truncate(10);
        aliases
    });
    Profile { color, aliases: aliases.filter(|aliases| !aliases.is_empty()) }
}

/// What an update changes; None leaves a column as it is.
#[derive(Debug, Clone, Default)]
pub struct UserChanges {
    pub name: Option<String>,
    /// Some(None) removes the password.
    pub password_hash: Option<Option<String>>,
    pub role: Option<Role>,
    pub status: Option<UserStatus>,
    pub restrictions: Option<Restrictions>,
    pub profile: Option<Profile>,
    pub last_login_at: Option<String>,
}

impl UserChanges {
    /// The record with these changes on top.
    pub fn applied_to(&self, user: &UserRecord) -> UserRecord {
        let mut next = user.clone();
        if let Some(name) = &self.name {
            next.name = name.clone();
        }
        if let Some(hash) = &self.password_hash {
            next.password_hash = hash.clone();
        }
        if let Some(role) = self.role {
            next.role = role;
        }
        if let Some(status) = self.status {
            next.status = status;
        }
        if let Some(restrictions) = &self.restrictions {
            next.restrictions = restrictions.clone();
        }
        if let Some(profile) = &self.profile {
            next.profile = profile.clone();
        }
        if let Some(at) = &self.last_login_at {
            next.last_login_at = Some(at.clone());
        }
        next
    }
}

const COLUMNS: &str = "id, name, password_hash, role, status, token_version, restrictions, profile, last_login_at, \
                       created_at, updated_at";

fn record(row: &Row<'_>) -> rusqlite::Result<UserRecord> {
    let json = |index: usize| -> rusqlite::Result<Option<Value>> {
        let text: String = row.get(index)?;
        Ok(serde_json::from_str(&text).ok())
    };
    let status: String = row.get(4)?;
    let version: i64 = row.get(5)?;
    Ok(UserRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        password_hash: row.get(2)?,
        role: normalize_role(&row.get::<_, String>(3)?),
        status: if status == "disabled" { UserStatus::Disabled } else { UserStatus::Active },
        token_version: version.max(0) as u64,
        restrictions: clean_restrictions(json(6)?.as_ref()),
        profile: clean_profile(json(7)?.as_ref()),
        last_login_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

pub struct UsersRepository {
    db: Arc<Db>,
}

impl UsersRepository {
    pub fn new(db: Arc<Db>) -> UsersRepository {
        UsersRepository { db }
    }

    pub fn count(&self) -> rusqlite::Result<usize> {
        self.db.with(|c| Users(c).count())
    }

    /// Everyone, in the order they were added.
    pub fn list(&self) -> rusqlite::Result<Vec<UserRecord>> {
        self.db.with(|c| Users(c).list())
    }

    pub fn by_id(&self, id: &str) -> rusqlite::Result<Option<UserRecord>> {
        self.db.with(|c| Users(c).by_id(id))
    }

    /// By name, case aside.
    pub fn by_name(&self, name: &str) -> rusqlite::Result<Option<UserRecord>> {
        self.db.with(|c| Users(c).by_name(name))
    }

    pub fn insert(&self, user: &UserRecord) -> rusqlite::Result<()> {
        self.db.with(|c| Users(c).insert(user))
    }

    pub fn update(&self, id: &str, changes: &UserChanges) -> rusqlite::Result<()> {
        self.db.with(|c| Users(c).update(id, changes))
    }

    pub fn remove(&self, id: &str) -> rusqlite::Result<()> {
        self.db.with(|c| Users(c).remove(id))
    }

    /// Every change of rights or password goes through here: the person's sessions end with it.
    pub fn bump_token_version(&self, id: &str) -> rusqlite::Result<()> {
        self.db.with(|c| Users(c).bump_token_version(id))
    }

    /// The first person, only while there is nobody: the check and the insert in one transaction, so two first-run
    /// requests at once cannot make two admins.
    pub fn insert_if_empty(&self, user: &UserRecord) -> rusqlite::Result<bool> {
        self.db.transaction(|t| {
            let users = Users(t);
            if users.count()? > 0 {
                return Ok(false);
            }
            users.insert(user)?;
            Ok(true)
        })
    }

    /// Runs `work` in one transaction: what it reads to decide and what it writes, with no other request's write in
    /// between (a rule over several people — always an admin, a name taken once — holds however many ask at once).
    /// Rolled back on any error; the API's own errors come out as they went in.
    pub fn transaction<T>(&self, work: impl FnOnce(&Users<'_>) -> Result<T, ApiError>) -> Result<T, ApiError> {
        let mut refused: Option<ApiError> = None;
        let done = self.db.transaction(|t| {
            work(&Users(t)).map_err(|error| {
                refused = Some(error);
                // Any error rolls the transaction back; the API's own goes out below in its place.
                rusqlite::Error::QueryReturnedNoRows
            })
        });
        done.map_err(|error| refused.take().unwrap_or_else(|| ApiError::from(error)))
    }
}

/// The `users` table on one connection: the repository's queries, and a transaction's.
pub struct Users<'c>(&'c Connection);

impl Users<'_> {
    pub fn count(&self) -> rusqlite::Result<usize> {
        self.0.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0)).map(|n| n as usize)
    }

    pub fn list(&self) -> rusqlite::Result<Vec<UserRecord>> {
        let mut statement = self.0.prepare(&format!("SELECT {COLUMNS} FROM users ORDER BY created_at, rowid"))?;
        let rows = statement.query_map([], record)?;
        rows.collect()
    }

    pub fn by_id(&self, id: &str) -> rusqlite::Result<Option<UserRecord>> {
        self.0.query_row(&format!("SELECT {COLUMNS} FROM users WHERE id = ?1"), params![id], record).optional()
    }

    pub fn by_name(&self, name: &str) -> rusqlite::Result<Option<UserRecord>> {
        let wanted = name.trim().to_lowercase();
        Ok(self.list()?.into_iter().find(|user| user.name.to_lowercase() == wanted))
    }

    pub fn insert(&self, user: &UserRecord) -> rusqlite::Result<()> {
        self.0
            .execute(
                &format!("INSERT INTO users ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
                params![
                    user.id,
                    user.name,
                    user.password_hash,
                    user.role.as_str(),
                    user.status.as_str(),
                    user.token_version as i64,
                    to_json(&user.restrictions),
                    to_json(&user.profile),
                    user.last_login_at,
                    user.created_at,
                    user.updated_at,
                ],
            )
            .map(|_| ())
    }

    pub fn update(&self, id: &str, changes: &UserChanges) -> rusqlite::Result<()> {
        let mut columns: Vec<&str> = Vec::new();
        let mut values: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(name) = &changes.name {
            columns.push("name");
            values.push(Box::new(name.clone()));
        }
        if let Some(hash) = &changes.password_hash {
            columns.push("password_hash");
            values.push(Box::new(hash.clone()));
        }
        if let Some(role) = changes.role {
            columns.push("role");
            values.push(Box::new(role.as_str()));
        }
        if let Some(status) = changes.status {
            columns.push("status");
            values.push(Box::new(status.as_str()));
        }
        if let Some(restrictions) = &changes.restrictions {
            columns.push("restrictions");
            values.push(Box::new(to_json(restrictions)));
        }
        if let Some(profile) = &changes.profile {
            columns.push("profile");
            values.push(Box::new(to_json(profile)));
        }
        if let Some(at) = &changes.last_login_at {
            columns.push("last_login_at");
            values.push(Box::new(at.clone()));
        }
        columns.push("updated_at");
        values.push(Box::new(now_iso()));
        let assignments: Vec<String> =
            columns.iter().enumerate().map(|(index, column)| format!("{column} = ?{}", index + 1)).collect();
        let sql = format!("UPDATE users SET {} WHERE id = ?{}", assignments.join(", "), columns.len() + 1);
        values.push(Box::new(id.to_string()));
        let parameters: Vec<&dyn ToSql> = values.iter().map(|value| value.as_ref()).collect();
        self.0.execute(&sql, parameters.as_slice()).map(|_| ())
    }

    pub fn remove(&self, id: &str) -> rusqlite::Result<()> {
        self.0.execute("DELETE FROM users WHERE id = ?1", params![id]).map(|_| ())
    }

    pub fn bump_token_version(&self, id: &str) -> rusqlite::Result<()> {
        self.0
            .execute(
                "UPDATE users SET token_version = token_version + 1, updated_at = ?1 WHERE id = ?2",
                params![now_iso(), id],
            )
            .map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn person(id: &str, name: &str, role: Role, hash: Option<&str>) -> UserRecord {
        UserRecord {
            id: id.into(),
            name: name.into(),
            password_hash: hash.map(str::to_string),
            role,
            status: UserStatus::Active,
            token_version: 0,
            restrictions: Restrictions::default(),
            profile: Profile::default(),
            last_login_at: None,
            created_at: now_iso(),
            updated_at: now_iso(),
        }
    }

    #[test]
    fn keeps_only_grants_the_catalog_knows_once_each() {
        let cleaned = clean_restrictions(Some(&json!({
            "allow": ["use:calls", "use:calls", "fly", 5],
            "deny": ["use:nonsense"],
        })));
        assert_eq!(cleaned, Restrictions { allow: Some(vec!["use:calls".into()]), deny: None });
        assert_eq!(clean_restrictions(Some(&json!("x"))), Restrictions::default());
        assert_eq!(serde_json::to_value(clean_restrictions(None)).unwrap(), json!({}));
    }

    #[test]
    fn keeps_a_colour_and_up_to_ten_names_the_person_is_called_by() {
        let profile = clean_profile(Some(&json!({
            "color": "#A1b2C3",
            "aliases": [" Аня ", "Аня", "", 7, "Анечка", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9"],
        })));
        assert_eq!(profile.color.as_deref(), Some("#A1b2C3"));
        let aliases = profile.aliases.unwrap();
        assert_eq!(aliases.len(), 10);
        assert_eq!(&aliases[..2], ["Аня", "Анечка"]);
        assert_eq!(clean_profile(Some(&json!({"color": "red", "aliases": []}))), Profile::default());
    }

    #[test]
    fn stores_and_reads_people_and_ends_their_sessions_by_version() {
        let users = UsersRepository::new(Arc::new(Db::memory()));
        let mut anya = person("u1", "Аня", Role::Member, Some("scrypt:a:b"));
        anya.restrictions = Restrictions { allow: None, deny: Some(vec!["use:calls".into()]) };
        users.insert(&anya).unwrap();
        assert_eq!(users.by_name("  аНЯ ").unwrap().map(|user| user.id), Some("u1".into()));
        assert_eq!(users.by_id("u1").unwrap().unwrap().restrictions, anya.restrictions);
        users
            .update("u1", &UserChanges { password_hash: Some(None), role: Some(Role::Guest), ..UserChanges::default() })
            .unwrap();
        users.bump_token_version("u1").unwrap();
        let after = users.by_id("u1").unwrap().unwrap();
        assert_eq!((after.role, after.password_hash, after.token_version), (Role::Guest, None, 1));
        assert!(!users.insert_if_empty(&person("u2", "Второй", Role::Admin, None)).unwrap());
        users.remove("u1").unwrap();
        assert!(users.insert_if_empty(&person("u2", "Второй", Role::Admin, None)).unwrap());
        assert_eq!(users.count().unwrap(), 1);
    }
}
