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

pub struct Caller {
    app: Arc<App>,
    pub zone: Zone,
    /// The address sign-in failures are counted by.
    pub source: String,
    /// The socket's address.
    pub peer: Option<SocketAddr>,
    pub headers: HeaderMap,
    authorization: Option<String>,
    /// Who signed, checked once: nobody, somebody, or a check that failed.
    principal: OnceLock<Result<Option<Principal>, ApiError>>,
}

impl FromRequestParts<Arc<App>> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
        let zone = zone_of(&parts.headers, peer, &app.config.gateway.debug_api_allow);
        Ok(Caller {
            app: app.clone(),
            zone,
            source: client_address(&parts.headers, peer),
            peer,
            headers: parts.headers.clone(),
            authorization: parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string),
            principal: OnceLock::new(),
        })
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
            principal: cell,
        }
    }

    /// Only from this computer (`Local`) or the home network too (`Home`).
    pub fn zone_at_least(&self, required: Zone) -> Result<(), ApiError> {
        if zone_allows(required, self.zone) {
            return Ok(());
        }
        Err(ApiError::forbidden(if required == Zone::Local {
            "Только с этого компьютера."
        } else {
            "Только из домашней сети."
        }))
    }

    /// Who signed the request, when anyone did (a token or a key), whatever the route. A check that failed reads as
    /// nobody here.
    pub fn principal(&self) -> Option<&Principal> {
        self.checked().as_ref().ok().and_then(Option::as_ref)
    }

    fn checked(&self) -> &Result<Option<Principal>, ApiError> {
        self.principal.get_or_init(|| self.app.access.try_authenticate(self.authorization.as_deref()))
    }

    /// A signed-in person or a key; from outside the home only when the owner opened it (PANEL_REMOTE). A sign-in
    /// that could not be checked is the server's error, not «sign in again».
    pub fn signed_in(&self) -> Result<&Principal, ApiError> {
        if self.zone == Zone::Remote && !self.app.config.gateway.panel_remote {
            return Err(ApiError::forbidden("Пульт вне дома выключен (PANEL_REMOTE)."));
        }
        match self.checked() {
            Ok(Some(principal)) => Ok(principal),
            Ok(None) => Err(ApiError::unauthorized("Нужно войти.")),
            Err(error) => Err(error.clone()),
        }
    }

    /// Signed in and holding the right.
    pub fn permitted(&self, permission: &str) -> Result<&Principal, ApiError> {
        let principal = self.signed_in()?;
        if !can(Some(principal), permission) {
            return Err(ApiError::forbidden(format!("Нет права: {}.", title_of(permission).to_lowercase())));
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
