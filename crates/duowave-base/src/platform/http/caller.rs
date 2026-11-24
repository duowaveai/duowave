//! Who is calling and from where — the Rust form of the Node gateway's AccessGuard. Every route is closed unless it
//! says otherwise; a handler takes a `Caller` and asks it, in this order, as the guard did:
//!
//! 1. `caller.zone_at_least(Zone::Home)?` — only from this computer or the home network (a route's @Zone);
//! 2. `caller.signed_in()?` / `caller.permitted(perm)?` — from outside the home only with PANEL_REMOTE, then a
//!    signed-in person or a key (401), then the right (403).
//!
//! A public route (@Public) asks none of it and may still read `caller.principal()`: who signed the request, when
//! anyone did.
//!
//! A sign-in that cannot be checked (the database failed) is the server's error (500) where a sign-in is required —
//! never a 401, which would make the panel drop its session — and nobody on a public route.
//!
//! A caller is checked against a `Gate`, not the whole gateway: the HTTP layer stands below every module
//! (gateway/layers.json), and the gateway fills the gate in (src/links/http.rs).
use std::any::Any;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap};

use super::api::ApiError;
use super::state::RouteState;
use super::zone::{client_address, zone_allows, zone_of, Zone};
use crate::core::events::Actor;
use crate::core::identity::permissions::title_of;
use crate::core::identity::{can, Principal};

/// A request a home sent over the link (src/helpers/link_serve.rs): which client of this computer it is, whether its
/// home key proved it this computer's own home, and who it named. Only the link server inserts it, as an extension —
/// never a header —, and a caller with it is outside the home whatever socket it came by.
#[derive(Clone, Debug)]
pub struct LinkOrigin {
    pub client: String,
    pub own_home: bool,
    pub home: crate::core::helpers::contract::HelperIdentity,
    pub gateway: crate::core::helpers::contract::HelperIdentity,
}

impl LinkOrigin {
    /// The principal of its requests: a key that grants only the computer's help (`home.models`), named by the home
    /// and the gateway.
    fn principal(&self) -> Principal {
        let name = match (self.home.name.trim(), self.gateway.name.trim()) {
            ("", "") => self.client.clone(),
            (home, "") => home.to_string(),
            ("", gateway) => gateway.to_string(),
            (home, gateway) => format!("{home} · {gateway}"),
        };
        Principal {
            kind: crate::core::identity::principal::PrincipalKind::ApiKey,
            id: format!("link-{}", self.client),
            name,
            role: None,
            grants: vec![crate::core::identity::permissions::HOME_MODELS.to_string()],
            denies: Vec::new(),
        }
    }
}

/// What a `Caller` asks of the gateway around it — who signed and from where — and nothing else.
pub trait Gate: Send + Sync + 'static {
    /// The networks besides this computer's that count as home (DEBUG_API_ALLOW).
    fn home_networks(&self) -> &[String];
    /// Whether the panel answers from outside the home (PANEL_REMOTE).
    fn panel_remote(&self) -> bool;
    /// The pair's key, when this gateway has a pair (src/placement).
    fn peer_key(&self) -> Option<&str>;
    /// Whether the home knows a person by this id.
    fn knows_person(&self, id: &str) -> bool;
    /// Who signed with this `Authorization`: nobody, somebody, or a check that failed (src/access).
    fn authenticate(&self, authorization: Option<&str>) -> Result<Option<Principal>, ApiError>;
    /// The gateway itself, for the modules that still take it whole (`crate::app::CallerApp`).
    fn as_any(&self) -> &dyn Any;
}

/// A request a helper at home sent this gateway over its link — its own request, `up` (src/helpers/link.rs; stage 7 of
/// docs/plans/home-setup-flow.md): its entry, and the person at the helper as its pass names them. Only the link's
/// `up` inserts it, as an extension — never a header —; a caller with it is at home (never this computer) and is that
/// person alone, whatever the request carries.
#[derive(Clone, Debug)]
pub struct UpOrigin {
    pub entry: String,
    pub principal: Principal,
}

/// The origin of a helper's own request (`up`) of the entry `entry`: the person its frame names, as a pass names them
/// (`person_principal`) — their role there (an unknown one is a guest's: never above an admin), their name, their id
/// when this home knows it. None when it names nobody (no role): such a request is refused, never let in as the helper
/// or as a key.
pub fn up_origin(gate: &dyn Gate, entry: &str, person: &serde_json::Value) -> Option<UpOrigin> {
    let field = |name: &str| person.get(name).and_then(serde_json::Value::as_str);
    let role = field("role").map(str::trim).filter(|role| !role.is_empty())?;
    let principal = person_principal(gate, field("name"), Some(role), field("id"));
    Some(UpOrigin { entry: entry.to_string(), principal })
}

pub struct Caller {
    gate: Arc<dyn Gate>,
    pub zone: Zone,
    /// The address sign-in failures are counted by.
    pub source: String,
    /// The socket's address.
    pub peer: Option<SocketAddr>,
    pub headers: HeaderMap,
    authorization: Option<String>,
    /// A home's request over the link.
    link: Option<LinkOrigin>,
    /// A helper's own request over its link (`up`).
    up: Option<UpOrigin>,
    /// Who signed, checked once: nobody, somebody, or a check that failed.
    principal: OnceLock<Result<Option<Principal>, ApiError>>,
}

impl<S: RouteState> FromRequestParts<S> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let gate = super::state::GateOf::gate(state);
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
        let link = parts.extensions.get::<LinkOrigin>().cloned();
        let up = parts.extensions.get::<UpOrigin>().cloned();
        // A home over the link is outside, whatever socket the link came by (a proxy on this computer). A helper's own
        // request is at home: its door lets in only a helper at home, whose pass serves only its home network.
        let zone = if link.is_some() {
            Zone::Remote
        } else if up.is_some() {
            Zone::Home
        } else {
            zone_of(&parts.headers, peer, gate.home_networks())
        };
        // A helper's own request has no socket of its own here: it counts by its entry, and no key it carries counts.
        let (source, peer, authorization) = match &up {
            Some(up) => (format!("up:{}", up.entry), None, None),
            None => (
                client_address(&parts.headers, peer),
                peer,
                parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string),
            ),
        };
        Ok(Caller {
            gate,
            zone,
            source,
            peer,
            headers: parts.headers.clone(),
            authorization,
            link,
            up,
            principal: OnceLock::new(),
        })
    }
}

/// The person a pass names — the computer's to a station with the pair's key, or a helper's own request over its link
/// (`up`) —: `helper_principal`, by their id only when this home knows it.
fn person_principal(gate: &dyn Gate, person: Option<&str>, role: Option<&str>, id: Option<&str>) -> Principal {
    let known = id.map(str::trim).filter(|id| {
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_graphic()) && gate.knows_person(id)
    });
    helper_principal(person, role, known)
}

/// «Помощник» — the computer's gateway with the pair's key, named with the person at the computer when the pass says
/// who it is. Its rights are that person's role's, as the pass says it (`HELPER_ROLE`; an unknown one is the least, a
/// guest); without a role — the computer's own calls, which ask with the key itself — a member's, never an admin's.
fn helper_principal(person: Option<&str>, role: Option<&str>, id: Option<&str>) -> Principal {
    use crate::core::identity::roles::{normalize_role, Role};
    let person: String = person.unwrap_or("").chars().filter(|c| !c.is_control()).take(40).collect();
    let name =
        if person.trim().is_empty() { "Турбо".to_string() } else { format!("Турбо ({})", person.trim()) };
    let role = role.map(|role| normalize_role(role.trim())).unwrap_or(Role::Member);
    Principal {
        kind: crate::core::identity::principal::PrincipalKind::User,
        // The person by their id in the home when the pass names one the home knows (docs/plans/home-and-devices.md,
        // stage 2): their desk and panel memory are theirs on every device; else the one shared row of the computer.
        id: id.map(str::to_string).unwrap_or_else(|| "helper".into()),
        name,
        role: Some(role),
        grants: role.info().grants,
        denies: Vec::new(),
    }
}

impl Caller {
    /// A caller for a test or a tool: from this computer, signed by `principal`.
    pub fn local(gate: Arc<dyn Gate>, principal: Option<Principal>) -> Caller {
        let cell = OnceLock::new();
        let _ = cell.set(Ok(principal));
        Caller {
            gate,
            zone: Zone::Local,
            source: "127.0.0.1".into(),
            peer: None,
            headers: HeaderMap::new(),
            authorization: None,
            link: None,
            up: None,
            principal: cell,
        }
    }

    /// The home that sent this request over the link, when one did.
    pub fn link(&self) -> Option<&LinkOrigin> {
        self.link.as_ref()
    }

    /// Only from this computer (`Local`) or the home network too (`Home`).
    pub fn zone_at_least(&self, required: Zone) -> Result<(), ApiError> {
        if zone_allows(required, self.zone) {
            return Ok(());
        }
        Err(ApiError::forbidden(if required == Zone::Local {
            crate::tr!("Только с этого компьютера.")
        } else {
            crate::tr!("Только из домашней сети.")
        }))
    }

    /// Who signed the request, when anyone did (a token or a key), whatever the route. A check that failed reads as
    /// nobody here.
    pub fn principal(&self) -> Option<&Principal> {
        self.checked().as_ref().ok().and_then(Option::as_ref)
    }

    fn checked(&self) -> &Result<Option<Principal>, ApiError> {
        self.principal.get_or_init(|| {
            // A home over the link: the link's own principal, whatever the request carries.
            if let Some(link) = &self.link {
                return Ok(Some(link.principal()));
            }
            // A helper at home asking for its person (`up`): that person, as its pass names them.
            if let Some(up) = &self.up {
                return Ok(Some(up.principal.clone()));
            }
            // The pair's key from the home network: the helper — the computer's gateway passing its panel's requests
            // to this station (src/placement/proxy.rs) — acts here with the rights of the role of who is at the
            // computer, under their name. Both headers count only beside the key. A name comes as UTF-8 («Мама»).
            if crate::platform::http::peer::from_peer(self, self.gate.peer_key()) {
                let header =
                    |name: &str| self.headers.get(name).and_then(|value| std::str::from_utf8(value.as_bytes()).ok());
                return Ok(Some(person_principal(
                    &*self.gate,
                    header(crate::platform::http::peer::HELPER_PERSON),
                    header(crate::platform::http::peer::HELPER_ROLE),
                    header(crate::platform::http::peer::HELPER_PERSON_ID),
                )));
            }
            self.gate.authenticate(self.authorization.as_deref())
        })
    }

    /// A signed-in person or a key; from outside the home only when the owner opened it (PANEL_REMOTE) — a home over
    /// the link aside, which reaches only `/home/v1`. A sign-in that could not be checked is the server's error, not
    /// «sign in again».
    pub fn signed_in(&self) -> Result<&Principal, ApiError> {
        if self.zone == Zone::Remote && !self.gate.panel_remote() && self.link.is_none() {
            return Err(ApiError::forbidden(crate::tr!("Пульт вне дома выключен (PANEL_REMOTE).")));
        }
        match self.checked() {
            Ok(Some(principal)) => Ok(principal),
            Ok(None) => Err(ApiError::unauthorized(crate::tr!("Нужно войти."))),
            Err(error) => Err(error.clone()),
        }
    }

    /// Signed in and holding the right.
    pub fn permitted(&self, permission: &str) -> Result<&Principal, ApiError> {
        let principal = self.signed_in()?;
        if !can(Some(principal), permission) {
            return Err(ApiError::forbidden(crate::tr!(
                "Нет права: {right}.",
                right = title_of(permission).to_lowercase()
            )));
        }
        Ok(principal)
    }

    /// The signed-in principal as the events' actor.
    pub fn actor(&self) -> Option<Actor> {
        self.principal().map(Actor::from)
    }

    /// The gate this caller was checked against.
    pub fn gate(&self) -> &Arc<dyn Gate> {
        &self.gate
    }
}
