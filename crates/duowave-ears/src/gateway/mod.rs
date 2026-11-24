//! The station link — the Station's adapter to the ear contract (src/ear): its protocol and its commands, the uniproxy
//! server and sessions, the relay to Yandex, our own session with Yandex, the journal of sessions, the station's own
//! HTTP and the debug API.

#[cfg(feature = "yandex-station")]
pub mod commands;
#[cfg(feature = "yandex-station")]
pub mod debug;
// The engine that shows and plays (keys, pictures, the streams relayed): every build has it, the panel's viewer too.
pub mod media;
#[cfg(feature = "yandex-station")]
pub mod observability;
#[cfg(feature = "yandex-station")]
pub mod protocol;
#[cfg(feature = "yandex-station")]
pub mod station_http;
#[cfg(feature = "yandex-station")]
pub mod uniproxy;
#[cfg(feature = "yandex-station")]
pub mod yandex;

#[cfg(feature = "yandex-station")]
crate::route_needs!(
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<crate::ear::mode::ModeService>,
    std::sync::Arc<uniproxy::SessionRegistry>,
    std::sync::Arc<crate::skills::SkillsService>,
    std::sync::Arc<crate::rights::Bus>,
    media::FolderMedia,
    std::sync::Arc<media::stream::StreamRelays>,
    std::sync::Arc<dyn media::AppMedia>,
    std::sync::Arc<dyn media::TurboMedia>
);
// The media's routes alone, without the Station's adapter.
#[cfg(not(feature = "yandex-station"))]
crate::route_needs!(
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<crate::ear::mode::ModeService>,
    std::sync::Arc<crate::skills::SkillsService>,
    std::sync::Arc<crate::rights::Bus>,
    media::FolderMedia,
    std::sync::Arc<media::stream::StreamRelays>,
    std::sync::Arc<dyn media::AppMedia>,
    std::sync::Arc<dyn media::TurboMedia>
);
