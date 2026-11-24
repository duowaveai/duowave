//! The state routes run on, without the gateway around them (docs/ARCHITECTURE.md, «Этажи»). A module's routes say
//! what they use — `Uses<Arc<AccessService>>` in a handler, `S: Provides<Arc<AccessService>>` on its `routes()` — and
//! the gateway provides it (src/links/http.rs). A module that names the whole gateway (`State<Arc<App>>`) can never
//! stand on a floor below it; one that names only what it uses can (gateway/layers.json, step В0 of
//! docs/plans/modular-gateway.md).
//!
//! Like axum's `FromRef` and `State`, but a trait of our own: the gateway's state is an `Arc<App>`, and the orphan rule
//! keeps `FromRef<Arc<App>>` from being implemented for the `Arc` of a service. This floor is a crate of its own: the
//! gateway's crate implements `Provides` for its `App` (its own type), and an `Arc` of a state provides what the state
//! provides (below).
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use super::caller::Gate;

/// The state gives a `T`: a service, the configuration, the gate a `Caller` is checked against.
pub trait Provides<T> {
    fn provide(&self) -> T;
}

/// A state behind a reference gives what the state gives: a test holding its gateway as `&Arc<App>`.
impl<S: Provides<T>, T> Provides<T> for &S {
    fn provide(&self) -> T {
        (**self).provide()
    }
}

/// A shared state gives what the state gives: the gateway's routes run on its `Arc<App>`.
impl<S: Provides<T>, T> Provides<T> for Arc<S> {
    fn provide(&self) -> T {
        (**self).provide()
    }
}

/// The gate a `Caller` is checked against (caller.rs): a shared gate is its own — the gateway's `Arc<App>`.
pub trait GateOf {
    fn gate(&self) -> Arc<dyn Gate>;
}

impl<G: Gate + 'static> GateOf for Arc<G> {
    fn gate(&self) -> Arc<dyn Gate> {
        self.clone()
    }
}

impl<S: GateOf> GateOf for &S {
    fn gate(&self) -> Arc<dyn Gate> {
        (**self).gate()
    }
}

/// A handler's `T` from the state: `Uses(access): Uses<Arc<AccessService>>`.
pub struct Uses<T>(pub T);

impl<T> Uses<T> {
    /// Straight from a state: a test calling a handler — `listen(Uses::of(&app), …)`.
    pub fn of<S: Provides<T>>(state: &S) -> Uses<T> {
        Uses(state.provide())
    }
}

impl<S, T> FromRequestParts<S> for Uses<T>
where
    S: Provides<T> + Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(Uses(state.provide()))
    }
}

/// The state any route runs on: one a `Caller` can be made from.
pub trait RouteState: GateOf + Clone + Send + Sync + 'static {}

impl<S> RouteState for S where S: GateOf + Clone + Send + Sync + 'static {}

/// A module's routes say what they use of the gateway, in the module's root: `crate::route_needs!(Arc<SpeechService>,
/// Arc<SettingsService>);` declares its `RouteNeeds` — a state that is a `RouteState` and provides each — for its
/// `routes<S: RouteNeeds>()`. The gateway gives each in src/links/http.rs.
#[macro_export]
macro_rules! route_needs {
    ($($ty:ty),+ $(,)?) => {
        /// What this module's routes use of the gateway (src/platform/http/state.rs).
        pub trait RouteNeeds: $crate::platform::http::RouteState $(+ $crate::platform::http::Provides<$ty>)+ {}

        impl<S> RouteNeeds for S where
            S: $crate::platform::http::RouteState $(+ $crate::platform::http::Provides<$ty>)+
        {
        }
    };
}
