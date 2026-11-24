//! The placement — the computer's part, added to «Дом»'s `placement` under its name: the moves between placements
//! (moves.rs; `duowave station setup` — setup.rs, cli.rs), the computer's watch over its stations' gateways
//! (watch.rs), the helper's pass to one (proxy.rs), the relay of its live channel to the computer's panels (relay.rs)
//! and of the computer's Telegram bot's messages to it (messages.rs), what a move carries (carry.rs), its routes
//! (routes.rs), and the computer's answer to what the placement asks of it (computer.rs).

pub use duowave_home::placement::*;

pub mod carry;
pub mod cli;
pub mod computer;
pub mod messages;
pub mod moves;
pub mod proxy;
pub mod relay;
pub mod routes;
pub mod setup;
pub mod watch;

pub use routes::router;

crate::route_needs!(
    std::sync::Arc<PlacementService>,
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<crate::storage::Db>,
    std::sync::Arc<crate::access::AccessService>,
    std::sync::Arc<AvailabilityService>,
    std::sync::Arc<crate::core::identity::home::Identity>,
    std::sync::Arc<crate::home::join::Joins>,
    std::sync::Arc<crate::plugins::files::PluginFiles>,
    std::sync::Arc<crate::secrets::SecretsService>,
    crate::core::events::EventBus,
    std::sync::Arc<watch::StationGateways>,
    std::sync::Arc<crate::stations::connect::station_connect::StationConnectService>,
    std::sync::Arc<moves::MovesService>,
    std::sync::Arc<crate::helpers::clients::Clients>,
    std::sync::Arc<crate::helpers::link_serve::LinkServer>,
);
