//! The pair's key and the helper's pass: how a request shows it comes from the computer this station is paired with
//! (`from_peer`), or from a person at that computer through its pass (`through_pass`, src/placement/proxy.rs). The key
//! itself is kept by the placement (src/placement/peer.rs); the caller checks a request against it.

use axum::http::header::AUTHORIZATION;
use subtle::ConstantTimeEq;

use super::{ApiError, Caller, Zone};
use crate::core::identity::principal::{Principal, PrincipalKind};
use crate::core::identity::roles::Role;

/// The header of the helper's pass: who is at the computer (src/placement/proxy.rs).
pub const HELPER_PERSON: &str = "x-duowave-helper";
/// The pass's other header: that person's role on the computer — admin, member or guest. The station believes it only
/// beside the pair's key: «Помощник» has that role's rights there, and without it a member's (platform/http/caller.rs).
/// The computer's own calls carry none: they ask with the pair's key itself (`from_peer`), not with a role's rights.
pub const HELPER_ROLE: &str = "x-duowave-helper-role";
/// Who at the computer, by their id in the home (docs/plans/home-and-devices.md, stage 2): the pass names the person so
/// that the centre keeps their own desk and panel memory, the same on every device of the home.
pub const HELPER_PERSON_ID: &str = "x-duowave-helper-id";

/// A request to the station as the person at the computer: their name and role — the station gives that role's
/// rights — and a person's own id, never a key's: the centre keeps their desk and panel memory under it.
pub fn as_person(principal: &Principal, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut request = request
        .header(HELPER_PERSON, principal.name.chars().filter(|c| !c.is_control()).collect::<String>())
        .header(HELPER_ROLE, helper_role(principal).as_str());
    if principal.kind == PrincipalKind::User {
        request = request.header(
            HELPER_PERSON_ID,
            principal.id.chars().filter(|c| c.is_ascii_graphic()).take(64).collect::<String>(),
        );
    }
    request
}

/// The role the station gives the person at the computer: a person's own; a key's (no role) — a guest's.
pub fn helper_role(principal: &Principal) -> Role {
    match (principal.kind, principal.role) {
        (PrincipalKind::User, Some(role)) => role,
        _ => Role::Guest,
    }
}

/// A person's request through the helper's pass, not the computer's own call: the pass names who is at the computer.
pub fn through_pass(caller: &Caller) -> bool {
    caller.headers.contains_key(HELPER_PERSON) || caller.headers.contains_key(HELPER_ROLE)
}

/// Whether the request carries the pair's key, from the home network.
pub fn from_peer(caller: &Caller, peer_key: Option<&str>) -> bool {
    let Some(expected) = peer_key.filter(|key| key.len() >= 16) else { return false };
    if !matches!(caller.zone, Zone::Local | Zone::Home) {
        return false;
    }
    let given = caller
        .headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

/// The pair's key, or a person with the right: who may act on the station's gateway.
pub fn peer_or_permitted(caller: &Caller, peer_key: Option<&str>, permission: &str) -> Result<bool, ApiError> {
    if from_peer(caller, peer_key) {
        return Ok(true);
    }
    caller.permitted(permission)?;
    Ok(false)
}
