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

pub mod apps;
pub mod character;
pub mod copy;
pub mod devices;
pub mod ear;
pub mod queue;
pub mod rooms;
pub mod telegram;

pub use devices::watch;

/// The home's routes: its devices and its one list of apps.
pub fn routes() -> axum::Router<std::sync::Arc<crate::app::App>> {
    devices::routes()
        .merge(apps::routes())
        .merge(rooms::routes())
        .merge(copy::routes())
        .merge(ear::routes())
        .merge(telegram::routes())
        .merge(character::routes())
}
