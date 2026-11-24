//! What the placement asks of the computer it runs on (crates/duowave-pc gives it; the station's build has none): its
//! stations as /health and the placement page show them, the safety net told to one, the window on one of them.

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::platform::http::ApiError;

/// The computer's side of the placement.
pub trait PlacementComputer: Send + Sync {
    /// /health's part of the computer: its stations (`station`, `stations`) and the home it is the Turbo of over the
    /// link (`centre`).
    fn health(&self) -> Value;
    /// The placement page's part of the computer: the station asked about (`station`, else the only one), every
    /// station, the move going now, the addresses a station would dial it at — only the ones there are.
    fn view(&self, station: Option<&str>) -> Map<String, Value>;
    /// The safety net on or off, told to the station picked.
    fn safety<'a>(&'a self, station: Option<&'a str>, on: bool) -> BoxFuture<'a, Result<(), ApiError>>;
    /// A station of this computer for its window on the keyboard plugged in here (midi/routes.rs).
    fn pair_of(&self, station: &str) -> crate::midi::routes::StationPair;
    /// Whether a station of this computer runs a gateway of its own: this computer is then its home's Турбо.
    fn turbo(&self) -> bool;
    /// What each station of this computer says of its state (`deviceState` of its /health), with `reachable`.
    fn device_states(&self) -> Vec<Value>;
}
