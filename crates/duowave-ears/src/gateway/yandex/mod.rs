//! Our own session with Yandex for a station (yandex_session.rs): speech in Alice's voice, the Yandex route, the
//! callbacks of Yandex's commands, Yandex's pushes.

pub mod yandex_session;

pub use yandex_session::{
    upstream_headers, Exchange, ExchangeHandler, StationCredentials, Synthesis, UpstreamJournal, YandexEvents,
    YandexSession, YandexSessionOptions,
};
