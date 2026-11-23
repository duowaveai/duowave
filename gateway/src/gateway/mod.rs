//! The station link: its protocol, the uniproxy server and sessions, the relay to Yandex, our own session with
//! Yandex, the modes, the journal of sessions, the station's own HTTP and the debug API.

pub mod debug;
pub mod media;
pub mod mode;
pub mod observability;
pub mod protocol;
pub mod station_http;
pub mod uniproxy;
pub mod yandex;

crate::route_needs!(
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<mode::ModeService>,
    std::sync::Arc<uniproxy::SessionRegistry>,
    std::sync::Arc<crate::skills::SkillsService>,
    std::sync::Arc<crate::bus::Bus>,
    std::sync::Arc<media::stream::StreamRelays>,
    std::sync::Arc<crate::plugins::PluginHost>,
    std::sync::Arc<crate::home::turbo_devices::HouseDevices>
);
