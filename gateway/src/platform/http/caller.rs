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
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap};

use super::api::ApiError;
use super::zone::{client_address, zone_allows, zone_of, Zone};
use crate::app::App;
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
    pub home: crate::helpers::contract::HelperIdentity,
    pub gateway: crate::helpers::contract::HelperIdentity,
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

pub struct Caller {
    app: Arc<App>,
    pub zone: Zone,
    /// The address sign-in failures are counted by.
    pub source: String,
    /// The socket's address.
    pub peer: Option<SocketAddr>,
    pub headers: HeaderMap,
    authorization: Option<String>,
    /// A home's request over the link.
    link: Option<LinkOrigin>,
    /// Who signed, checked once: nobody, somebody, or a check that failed.
    principal: OnceLock<Result<Option<Principal>, ApiError>>,
}

impl FromRequestParts<Arc<App>> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
        let link = parts.extensions.get::<LinkOrigin>().cloned();
        // A home over the link is outside, whatever socket the link came by (a proxy on this computer).
        let zone = if link.is_some() {
            Zone::Remote
        } else {
            zone_of(&parts.headers, peer, &app.config.gateway.debug_api_allow)
        };
        Ok(Caller {
            app: app.clone(),
            zone,
            source: client_address(&parts.headers, peer),
            peer,
            headers: parts.headers.clone(),
            authorization: parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string),
            link,
            principal: OnceLock::new(),
        })
    }
}

/// «Помощник» — the computer's gateway with the pair's key, named with the person at the computer when the pass says
/// who it is. Its rights are that person's role's, as the pass says it (`HELPER_ROLE`; an unknown one is the least, a
/// guest); without a role — the computer's own calls, which ask with the key itself — a member's, never an admin's.
fn helper_principal(person: Option<&str>, role: Option<&str>) -> Principal {
    use crate::core::identity::roles::{normalize_role, Role};
    let person: String = person.unwrap_or("").chars().filter(|c| !c.is_control()).take(40).collect();
    let name =
        if person.trim().is_empty() { "Турбо".to_string() } else { format!("Турбо ({})", person.trim()) };
    let role = role.map(|role| normalize_role(role.trim())).unwrap_or(Role::Member);
    Principal {
        kind: crate::core::identity::principal::PrincipalKind::User,
        id: "helper".into(),
        name,
        role: Some(role),
        grants: role.info().grants,
        denies: Vec::new(),
    }
}

impl Caller {
    /// A caller for a test or a tool: from this computer, signed by `principal`.
    pub fn local(app: Arc<App>, principal: Option<Principal>) -> Caller {
        let cell = OnceLock::new();
        let _ = cell.set(Ok(principal));
        Caller {
            app,
            zone: Zone::Local,
            source: "127.0.0.1".into(),
            peer: None,
            headers: HeaderMap::new(),
            authorization: None,
            link: None,
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
            // The pair's key from the home network: the helper — the computer's gateway passing its panel's requests
            // to this station (src/placement/proxy.rs) — acts here with the rights of the role of who is at the
            // computer, under their name. Both headers count only beside the key. A name comes as UTF-8 («Мама»).
            if crate::placement::peer::from_peer(self, self.app.placement.peer_key()) {
                let header =
                    |name: &str| self.headers.get(name).and_then(|value| std::str::from_utf8(value.as_bytes()).ok());
                return Ok(Some(helper_principal(
                    header(crate::placement::peer::HELPER_PERSON),
                    header(crate::placement::peer::HELPER_ROLE),
                )));
            }
            self.app.access.try_authenticate(self.authorization.as_deref())
        })
    }

    /// A signed-in person or a key; from outside the home only when the owner opened it (PANEL_REMOTE) — a home over
    /// the link aside, which reaches only `/home/v1`. A sign-in that could not be checked is the server's error, not
    /// «sign in again».
    pub fn signed_in(&self) -> Result<&Principal, ApiError> {
        if self.zone == Zone::Remote && !self.app.config.gateway.panel_remote && self.link.is_none() {
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

    pub fn app(&self) -> &Arc<App> {
        &self.app
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, Env};
    use crate::placement::peer::{HELPER_PERSON, HELPER_ROLE};

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    /// The station's gateway (the computer's build playing it) with the pair's key in its .env.
    fn station() -> Arc<App> {
        let env = Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station"), ("PEER_KEY", PAIR)]);
        App::build(Config::for_tests(&env)).unwrap()
    }

    /// A GET from the home network with these headers.
    async fn get(app: &Arc<App>, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
        }
        let mut request = request.body(HttpBody::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_link_caller_is_remote_whatever_socket_it_came_by() {
        let app = station();
        let origin = LinkOrigin {
            client: "link:0123456789ab".into(),
            own_home: false,
            home: crate::helpers::contract::HelperIdentity { id: "h1".into(), name: "Дача".into() },
            gateway: crate::helpers::contract::HelperIdentity { id: "g1".into(), name: "Колонка".into() },
        };
        let mut request = Request::builder().uri("/home/v1/status").body(HttpBody::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
        request.extensions_mut().insert(origin);
        let (mut parts, _) = request.into_parts();
        let caller = Caller::from_request_parts(&mut parts, &app).await.unwrap();
        assert_eq!(caller.zone, Zone::Remote, "a loopback socket does not make a home's request this computer's");
        assert!(caller.zone_at_least(Zone::Home).is_err());
        assert_eq!(caller.link().map(|link| link.client.as_str()), Some("link:0123456789ab"));
        // Its own principal: the computer's help only, named by its home and gateway; PANEL_REMOTE does not stop it.
        let principal = caller.signed_in().unwrap();
        assert_eq!((principal.id.as_str(), principal.name.as_str()), ("link-link:0123456789ab", "Дача · Колонка"));
        assert!(caller.permitted("home.models").is_ok());
        assert_eq!(caller.permitted("station.view").unwrap_err().status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_helper_has_the_rights_of_the_role_the_pass_names_and_a_members_without_one() {
        let app = station();
        let pair = format!("Bearer {PAIR}");
        let me = |role: Option<&'static str>| {
            let mut headers = vec![("authorization", pair.as_str()), (HELPER_PERSON, "Мама")];
            headers.extend(role.map(|role| (HELPER_ROLE, role)));
            headers
        };

        // The computer's own call: no role — a member, never an admin.
        let (status, body) = get(&app, "/api/v1/auth/me", &[("authorization", pair.as_str())]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["message"]["role"], "member");
        assert_eq!(body["message"]["name"], "Турбо");
        let (_, body) = get(&app, "/api/v1/auth/me", &me(Some("admin"))).await;
        assert_eq!(body["message"]["role"], "admin");
        assert_eq!(body["message"]["name"], "Турбо (Мама)", "the name as it came, in UTF-8");
        let (_, body) = get(&app, "/api/v1/auth/me", &me(Some("owner"))).await;
        assert_eq!(body["message"]["role"], "guest", "an unknown role is the least");

        // A member at the computer sees what a member sees here: not the journal, the people, the history.
        for uri in ["/api/v1/requests", "/api/v1/users", "/api/v1/audit"] {
            let (status, body) = get(&app, uri, &me(Some("member"))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
            assert_eq!(get(&app, uri, &me(None)).await.0, StatusCode::FORBIDDEN, "{uri} without a role");
            let (status, body) = get(&app, uri, &me(Some("admin"))).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
        assert_eq!(get(&app, "/api/v1/availability", &me(Some("member"))).await.0, StatusCode::OK);
        assert_eq!(get(&app, "/api/v1/availability", &me(Some("guest"))).await.0, StatusCode::FORBIDDEN);

        // The role counts only beside the pair's key.
        let alone = [(HELPER_ROLE, "admin"), (HELPER_PERSON, "Мама")];
        assert_eq!(get(&app, "/api/v1/requests", &alone).await.0, StatusCode::UNAUTHORIZED);
        let wrong = [("authorization", "Bearer not-the-pairs-key-0123456789"), (HELPER_ROLE, "admin")];
        assert_ne!(get(&app, "/api/v1/requests", &wrong).await.0, StatusCode::OK);
    }
}
