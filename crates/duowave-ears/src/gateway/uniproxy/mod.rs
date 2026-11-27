//! The station's uniproxy session: the WebSocket at UNIPROXY_PATH (uniproxy_server.rs), the live sessions
//! (session_registry.rs), answered by us (station_session.rs: voice requests, spoken answers, the Yandex route) or
//! relayed to Yandex (relay_session.rs, with the requests' trips read off the frames).

pub mod device_tag;
/// A Station that is an ear of the centre: its socket goes on there (was in src/home/ear.rs).
pub mod ear_bridge;
// What the station says at once with no way out, recorded while it was online (a tester's Mini 2, 22.11).
pub mod offline_phrase;
pub mod relay_session;
pub mod relay_trips;
pub mod rescue;
pub mod session_registry;
pub mod spoken_answer;
pub mod station_session;
pub mod summaries;
pub mod uniproxy_server;
pub mod voice_request;
pub mod yandex_relay;

pub use session_registry::{AnySession, SessionRegistry};
pub use station_session::{SessionDeps, SessionSnapshot, StationSession, StationThinking};
