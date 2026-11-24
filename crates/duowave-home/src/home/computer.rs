//! What the home asks of the computer it runs on (crates/duowave-pc gives it; the station's build has none): the home
//! as this computer sees it when it is a Турбо — its centre elsewhere (a station with a gateway of its own), the home
//! it helps over the link —, the voice waiting here for the owner, and a station of its own joining as the centre's ear.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use super::Parts;
use crate::characters::CharactersService;
use crate::core::events::Actor;
use crate::platform::http::ApiError;

/// The computer's side of the home.
pub trait ComputerHome: Send + Sync {
    /// The home's devices when this computer is a Турбо: its centre's list (or what it knows while the centre is away),
    /// or the home it helps over the link; None — this computer keeps a home of its own.
    fn turbo_devices<'a>(&'a self, parts: &'a Parts) -> BoxFuture<'a, Option<Value>>;
    /// The centre elsewhere asked `path` through the pass: None — no such centre, or no answer.
    fn ask_centre<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<Value>>;
    /// The centre elsewhere's address (empty without its record); None — no centre elsewhere.
    fn centre_address(&self) -> Option<String>;
    /// The stations whose voice goes to a gateway of their own: not this centre's ears.
    fn elsewhere(&self) -> HashSet<String>;
    /// The voice waiting here for the owner (this computer is the home's centre).
    fn voice_waiting<'a>(&'a self, characters: &'a Arc<CharactersService>) -> BoxFuture<'a, Option<Value>>;
    /// The owner's answer to the voice waiting here; None — nothing waits here.
    fn answer_voice<'a>(&'a self, answer: &'a str, actor: Option<Actor>)
        -> BoxFuture<'a, Option<Result<(), ApiError>>>;
    /// A station this computer put DuoWave on joins the home as the centre's ear.
    fn join_station<'a>(&'a self, station: &'a str) -> BoxFuture<'a, Result<(), ApiError>>;
    /// The home this computer is the Turbo of over the link, as /health tells the panel.
    fn link_centre(&self, parts: &Parts) -> Option<Value>;
}
