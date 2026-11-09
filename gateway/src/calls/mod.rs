//! Our own calls, without Yandex (src/calls): the hub and its parties — the call page (a browser over WebRTC), the
//! station's own client over HTTP with its microphone bridge, the simulator, SIP phones through Asterisk, a LiveKit
//! room — and voice messages through a Telegram bot. For a PC (feature `pc`); WebRTC and LiveKit are features of
//! their own (`webrtc`, `livekit`), and without them their pages say they are not part of the build.
//!
//! What other modules call: `CallsService` (in `App` as `calls`) with its `hub` (`CallHub`: parties, calls,
//! `on(…)` for its events) and `telegram` (`TelegramMessenger`). The bot, SIP and LiveKit are set up in the panel
//! (src/connectors): the service reads them through `CallsConnectors` and follows each change (`refresh`).

pub mod call_hub;
pub mod calls_controller;
pub mod calls_service;
pub mod http_stations;
pub mod livekit;
pub mod opus;
pub mod page;
pub mod sip;
pub mod socket_endpoint;
pub mod station_mic;
pub mod telegram;
#[cfg(feature = "webrtc")]
pub mod webrtc_endpoint;

pub use call_hub::{
    Call, CallEndpoint, CallError, CallEvent, CallEventType, CallHub, CallSnapshot, CallState, EndpointKind, HubEvent,
    Party, FRAME_SAMPLES,
};
pub use calls_controller::router;
pub use calls_service::{CallsConnectors, CallsPart, CallsService, NoCallsConnectors, StationTrust, CALLS_WS_PATH};
pub use livekit::Invite;
pub use opus::{CallsOpus, SpeechOpus};
pub use telegram::{Contact, IncomingMessage, LinkButton, Stranger, TelegramMessenger};
