//! The home and its devices (docs/plans/home-and-devices.md): one home, one centre — the device that decides who
//! answers, keeps the home and the panel —, the ears that hear for it (a station, a computer's microphone) and its
//! Turbo, the muscles (src/helpers). People see the devices by their names with «Центр» and «Турбо» marks and a
//! «слышит» sign; «ухо» is a word of the schemes and of this code only.
//!
//! - devices.rs — `GET /api/v1/home/devices`: the devices of the home with their roles and state, as the centre sees
//!   them, and `home.devices.changed` when any of it moves. A computer that is the home's Turbo asks its centre for
//!   it through the pass (`/station/<key>/…`, src/placement/proxy.rs): it has no home of its own.
//! - apps.rs — `GET /api/v1/home/apps`: the one list «Установлено», each package with the device it runs on.
//! - character.rs — the home's character told to the Турбо that speaks for it: its voice is the character's; and
//!   `/api/v1/home/voice`, the model of that voice on the Турбо, its plan and the owner's choice.
//! - telegram.rs — the home's Telegram kept at the centre and handed to the Турбо that runs the bot.
//! - rooms.rs — what may differ by room: Alice's mode, the volume, who answers there.
//! - copy.rs, queue.rs — the home's copy on a device that can answer by itself, and its changes made while the
//!   centre was away.
//! - ear.rs — a second station as an ear of the centre, and «Сделать центром».
//! - join.rs, join_say.rs — into the home by a code: a Turbo or an ear asks the centre, the centre tells a code of six
//!   digits (aloud, in a window, on stdout) and shows it to the admins with «Разрешить».
//! - computer.rs — what the home asks of the computer it runs on (crates/duowave-pc: «Поиск дома», the home as a
//!   Турбо sees it, the voice waiting there).
//! - status.rs — «Состояние дома»: one look at what the home needs, a row each with the one thing that fixes it.
//! - turbo_devices.rs — the home's devices: what is plugged into this gateway and into every Турбо, as one
//!   `device/<kind>` on the bus each (docs/plans/turbo-devices.md); `/api/v1/home/plugged*`, the brain's tools.

pub mod apps;
pub mod character;
pub mod computer;
pub mod copy;
pub mod devices;
pub mod ear;
pub mod join;
pub mod join_say;
pub mod queue;
pub mod rooms;
pub mod status;
pub mod telegram;
pub mod turbo_devices;

pub use devices::watch;

use std::sync::Arc;

/// What the home's centre works with: its configuration and database, the bus, the secrets and settings, who this
/// gateway is, the helpers and the Турбо, the packages and their catalog, the characters, the stations and their
/// presence, the placement, the connectors' settings, the requests to come in by a code, the sessions, the voice, the
/// modes, the health and the brain — and on a computer what it adds (`computer`: its stations, models, voice lab, link). The gateway makes it
/// (src/links/http.rs); the watches get it through the gateway's weak handle (app.rs), as they always held the gateway.
#[derive(Clone)]
pub struct Parts {
    pub config: Arc<crate::config::Config>,
    pub db: Arc<crate::storage::Db>,
    pub events: crate::core::events::EventBus,
    pub secrets: Arc<crate::secrets::SecretsService>,
    pub settings: Arc<crate::settings::SettingsService>,
    pub identity: Arc<crate::core::identity::home::Identity>,
    pub helpers: Arc<crate::helpers::Helpers>,
    pub plugins: Arc<crate::plugins::PluginHost>,
    pub catalog: Arc<crate::plugins::PackageCatalog>,
    pub characters: Arc<crate::characters::CharactersService>,
    pub stations: Arc<crate::stations::StationsService>,
    pub presence: Arc<crate::stations::PresenceService>,
    pub placement: Arc<crate::placement::PlacementService>,
    pub connector_settings: Arc<crate::connectors::settings::ConnectorSettings>,
    pub joins: Arc<crate::home::join::Joins>,
    #[cfg(feature = "yandex-station")]
    pub sessions: Arc<crate::gateway::uniproxy::SessionRegistry>,
    pub voice: Arc<crate::speech::VoiceService>,
    pub modes: Arc<crate::ear::mode::ModeService>,
    pub health: Arc<crate::health_status::HealthStatusService>,
    pub brain: Arc<crate::dialog::brain::BrainService>,
    /// The computer it runs on — its stations, models, voice lab and link (crates/duowave-pc); none on the station.
    pub computer: Option<Arc<dyn computer::ComputerHome>>,
}

crate::route_needs!(
    Parts,
    Arc<crate::characters::CharactersService>,
    Arc<crate::config::Config>,
    Arc<crate::core::identity::home::Identity>,
    Arc<crate::ear::mode::ModeService>,
    Arc<crate::placement::PlacementService>,
    Arc<crate::secrets::SecretsService>,
    Arc<crate::settings::SettingsService>,
    Arc<crate::stations::StationsService>,
    Arc<crate::storage::Db>,
    crate::core::events::EventBus,
    Arc<turbo_devices::HouseDevices>,
);

/// The home's routes: its devices and its one list of apps.
pub fn routes<S: crate::home::RouteNeeds>() -> axum::Router<S> {
    devices::routes()
        .merge(apps::routes())
        .merge(rooms::routes())
        .merge(copy::routes())
        .merge(telegram::routes())
        .merge(character::routes())
        .merge(join::routes())
        .merge(status::routes())
        .merge(turbo_devices::routes::routes())
}
