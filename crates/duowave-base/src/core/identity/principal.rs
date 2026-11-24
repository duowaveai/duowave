//! Who acts: a person signed in to the panel, the household speaking to the station, an API key of a script or an
//! agent, or the gateway itself (a reminder it says, a scenario it runs). Every check of rights takes one.

use serde::{Deserialize, Serialize};

use super::roles::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrincipalKind {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "household")]
    Household,
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "system")]
    System,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
            PrincipalKind::Household => "household",
            PrincipalKind::ApiKey => "apiKey",
            PrincipalKind::System => "system",
        }
    }
}

/// The admin's per-person edits of a role: extra grants and denials; a denial always wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restrictions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub kind: PrincipalKind,
    /// A user's id, an API key's id, `household` or `system`.
    pub id: String,
    pub name: String,
    /// People have a role; a key carries its own grants instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    pub grants: Vec<String>,
    pub denies: Vec<String>,
}

/// A person: the role's grants with their restrictions on top.
pub fn user_principal(id: &str, name: &str, role: Role, restrictions: Option<&Restrictions>) -> Principal {
    let mut grants = role.info().grants;
    grants.extend(restrictions.and_then(|r| r.allow.clone()).unwrap_or_default());
    Principal {
        kind: PrincipalKind::User,
        id: id.to_string(),
        name: name.to_string(),
        role: Some(role),
        grants,
        denies: restrictions.and_then(|r| r.deny.clone()).unwrap_or_default(),
    }
}

/// The voice of the house when nobody is recognised: the role the admin gave the household (a member by default,
/// which is how the station worked before people existed) and the household's own restrictions.
pub fn household_principal(role: Role, restrictions: Option<&Restrictions>) -> Principal {
    let mut grants = role.info().grants;
    grants.extend(restrictions.and_then(|r| r.allow.clone()).unwrap_or_default());
    Principal {
        kind: PrincipalKind::Household,
        id: "household".into(),
        name: "Дом".into(),
        role: Some(role),
        grants,
        denies: restrictions.and_then(|r| r.deny.clone()).unwrap_or_default(),
    }
}

/// A script's or an agent's key: exactly the grants the admin gave it.
pub fn api_key_principal(id: &str, name: &str, grants: &[String]) -> Principal {
    Principal {
        kind: PrincipalKind::ApiKey,
        id: id.to_string(),
        name: name.to_string(),
        role: None,
        grants: grants.to_vec(),
        denies: Vec::new(),
    }
}

/// The gateway itself: reminders it says, its own maintenance.
pub fn system() -> Principal {
    Principal {
        kind: PrincipalKind::System,
        id: "system".into(),
        name: "Шлюз".into(),
        role: None,
        grants: vec!["*".into()],
        denies: Vec::new(),
    }
}
