//! The one answer to "may they": a denial that covers the permission wins, then a grant that covers it; nothing
//! else passes. Used by the HTTP checks, the router (a capability by voice), the model's tools and scenarios.

use super::permissions::{all_permissions, covers};
use super::principal::Principal;

pub fn can(principal: Option<&Principal>, permission: &str) -> bool {
    let Some(principal) = principal else { return false };
    if principal.denies.iter().any(|deny| covers(deny, permission)) {
        return false;
    }
    principal.grants.iter().any(|grant| covers(grant, permission))
}

/// Everything a principal may do, spelled out: the panel hides what the role cannot touch.
pub fn effective_permissions(principal: &Principal) -> Vec<String> {
    all_permissions().into_iter().filter(|permission| can(Some(principal), permission)).collect()
}

/// The panel's actions a principal may take (no capabilities): the compact list `me` returns.
pub fn panel_permissions(principal: &Principal) -> Vec<String> {
    effective_permissions(principal).into_iter().filter(|permission| !permission.starts_with("use:")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::{api_key_principal, system, user_principal, Restrictions};
    use crate::core::identity::roles::Role;

    fn person(role: Role, restrictions: Option<Restrictions>) -> Principal {
        user_principal("u", "Person", role, restrictions.as_ref())
    }

    #[test]
    fn gives_each_role_its_rights() {
        let admin = person(Role::Admin, None);
        let member = person(Role::Member, None);
        let guest = person(Role::Guest, None);
        assert!(can(Some(&admin), "users.manage"));
        assert!(can(Some(&member), "station.say"));
        assert!(!can(Some(&member), "station.mode"));
        assert!(can(Some(&member), "use:calls"));
        assert!(can(Some(&guest), "use:music"));
        assert!(!can(Some(&guest), "use:calls"));
        assert!(!can(Some(&guest), "use:smart_home"));
        assert!(!can(Some(&guest), "station.view"));
        assert_eq!(panel_permissions(&member), vec!["station.view", "station.say", "connectors.view", "mcp.use"]);
        assert!(!can(Some(&guest), "mcp.use"), "the servers' tools are for the household, not for guests");
    }

    #[test]
    fn restrictions_narrow_and_widen_a_role_a_denial_always_winning() {
        let kid = person(Role::Member, Some(Restrictions { allow: None, deny: Some(vec!["use:calls".into()]) }));
        assert!(!can(Some(&kid), "use:calls"));
        assert!(can(Some(&kid), "use:music"));
        let trusted = person(Role::Guest, Some(Restrictions { allow: Some(vec!["use:calls".into()]), deny: None }));
        assert!(can(Some(&trusted), "use:calls"));
        let both = person(
            Role::Admin,
            Some(Restrictions { allow: Some(vec!["use:calls".into()]), deny: Some(vec!["use:calls".into()]) }),
        );
        assert!(!can(Some(&both), "use:calls"));
        assert!(can(Some(&both), "models.manage"));
    }

    #[test]
    fn a_key_has_exactly_its_grants_the_gateway_everything_nobody_nothing() {
        let key = api_key_principal("k", "script", &["station.say".into()]);
        assert!(can(Some(&key), "station.say"));
        assert!(!can(Some(&key), "station.view"));
        assert!(can(Some(&system()), "users.manage"));
        assert!(!can(None, "station.view"));
    }
}
