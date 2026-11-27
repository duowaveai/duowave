//! The ear contract (docs/plans/modular-gateway.md, «Договор уха»; docs/plans/home-and-devices.md, «Ухо»): an ear is
//! any device with a microphone that hears for the home — Yandex's Station, the computer's microphone, a box of one's
//! own. The brain and the skills speak to an ear only through this: what it says of itself (`state`), what it can be
//! asked to do (`Command`), what its player plays (`MediaItem`), which ear hears the household (`input`), how the home
//! answers its ears (`mode`: Яндекс, Шлюз, Свой) and the word that switches a character (`word`). An ear's adapter —
//! the Station's is src/gateway, the floor «Колонка» — says it all in the ear's own words.

pub mod centre;
pub mod command;
/// What each kind of ear can do, in the door's words (docs/plans/audio-thinking-contract.md, «Hands»).
pub mod hands;
pub mod input;
pub mod media;
pub mod mode;
/// A device's own sound for the gateway's voice channel (stage 3, «К2», «К3»).
pub mod port;
pub mod state;
pub mod word;

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::core::capabilities::Intent;

pub use command::{AlarmSpec, CalendarDate, Command, TimerSpec};
pub use media::MediaItem;
pub use state::DeviceState;

/// The ear a tool's call answers for: a tool that acts (plays, speaks — docs/plans/plugins.md, «Инструмент плагина
/// не только находит, но и делает») adds its commands to that answer. None in the panel's check chat.
#[derive(Clone)]
pub struct EarAct {
    pub device: String,
    /// Where the ear fetches our /media.
    pub media_base_url: Option<String>,
    /// Adds commands to the answer: false when the answer went out already.
    pub add: Arc<dyn Fn(Vec<Command>) -> bool + Send + Sync>,
}

impl std::fmt::Debug for EarAct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EarAct").field("device", &self.device).finish_non_exhaustive()
    }
}

/// The turn timer («Время вышло!»): it stands apart from the scheduler (`schedule.rs`) and comes first (the owner's
/// decision 9, 12.11.2026) — it is small, and games show it at once. This is its way to the station: «время вышло» is
/// the answer to a push, so the plugin's answer to it plays, listens after and goes on as a phrase's answer does.
pub trait TurnStation: Send + Sync {
    /// Runs our `intent` as the answer to a push on the device's live station. `still` is asked when the station
    /// calls back: false, and nothing is answered. False when nothing was spoken to its end.
    fn ask(
        &self,
        device: &str,
        intent: Intent,
        label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> BoxFuture<'static, bool>;
    /// The person's words are being heard on the device now.
    fn hearing(&self, device: &str) -> bool;
    /// An alarm or a timer rings on the device.
    fn ringing(&self, device: &str) -> bool;
    /// Someone is with the station or it plays: the workshop's «Готово» waits (workshop/voice.rs).
    fn busy(&self, device: &str) -> bool {
        self.hearing(device) || self.ringing(device)
    }
}

/// The Station's sessions (src/gateway/uniproxy), as the Yandex connector reads them.
pub trait StationSessions: Send + Sync {
    /// Sessions with a station open now.
    fn open(&self) -> usize;
    /// Whether one of them has our own session with Yandex up (gateway mode).
    fn yandex_connected(&self) -> bool;
}

/// The station side of the schedules (the Station's session registry): speaking on an ear when the time comes.
pub trait ScheduleStation: Send + Sync {
    /// Says `text` on the device (`*`: whichever is connected); false when no station could.
    fn say(&self, device: &str, text: String) -> BoxFuture<'static, bool>;
    /// It rings, or someone talks to it (a conversation going on, words being heard): speech waits.
    fn busy(&self, device: &str) -> bool;
    /// The station's time zone as it said it (the newest connected), if one is.
    fn timezone(&self) -> Option<String>;
}
