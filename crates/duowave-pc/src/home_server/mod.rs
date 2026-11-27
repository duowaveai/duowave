//! The home server — the computer's part, added to «Дом»'s `home_server` under its name: `/home/v1` (serve.rs) with
//! its devices' letters (devices.rs), the voice plan for a character's voice here (voice_plan.rs), and «Что даю
//! колонке».

use std::sync::Arc;

use axum::routing::get;
use serde_json::Value;

pub use duowave_home::home_server::*;

use crate::platform::http::{ok, ApiResult, Caller};

pub mod devices;
pub mod serve;
/// `/home/v1/systems/call`: this computer's work systems for the hub (docs/plans/work-systems.md, stage 9).
pub mod systems;
pub mod voice_plan;

crate::route_needs!(
    serve::Door,
    Arc<crate::placement::PlacementService>,
    Arc<HomeServerService>,
    Arc<crate::config::Config>,
    Arc<crate::skills::SkillsService>,
    Arc<crate::dialog::brain::BrainService>,
    Arc<crate::plugins::PackageCatalog>,
    Arc<crate::plugins::PluginHost>,
    Arc<crate::voice_lab::VoiceLabService>,
    Arc<crate::characters::CharactersService>,
    Arc<crate::models::ModelsService>,
    Arc<crate::core::identity::home::Identity>,
    Arc<crate::connectors::settings::ConnectorSettings>,
    Option<Arc<crate::plugins::history::History>>,
    serve::OwnBot,
    Arc<crate::helpers::builtin::BuiltinHelper>,
    Arc<crate::speech::VoiceService>,
    Arc<crate::helpers::clients::Clients>,
    Arc<crate::helpers::link_serve::LinkServer>,
    Arc<crate::helpers::Helpers>,
    Arc<crate::home::turbo_devices::HouseDevices>,
    Arc<crate::settings::SettingsService>,
    Arc<crate::bus::Bus>,
    Arc<crate::midi::MidiService>,
    Arc<crate::talk::systems::Systems>
);

/// The home server's routes with the computer's own: `/home/v1` and «Что даю колонке».
pub fn router<S>() -> axum::Router<S>
where
    S: crate::home_server::RouteNeeds + duowave_home::home_server::RouteNeeds,
{
    duowave_home::home_server::router::<S>()
        .merge(serve::router())
        .merge(systems::router())
        .route("/api/v1/home-server/served", get(served))
}

/// The helper's «Что даю колонке»: how many times and how fast it gave the station hearing, voice and the model
/// today.
pub async fn served(caller: Caller) -> ApiResult<Value> {
    caller.permitted(crate::core::identity::permissions::STATION_VIEW)?;
    Ok(ok(serve::served_today()))
}
